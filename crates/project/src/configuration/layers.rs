use std::{
    collections::{BTreeMap, BTreeSet},
    env::current_dir,
    fmt::Display,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
};

use instar_lint::{RuleIdentity, RuleProducer};

use instar_luau::{
    ConfigurationContext, ConfigurationInput, ConfigurationKey, ConfigurationLimits,
    ConfigurationValue, LanguageFeatures,
    flags::{FlagValue, LanguageProfile},
};

use instar_shared::{
    Completion, ContextIdentity, Diagnostic, DiagnosticCategory, Event, EventPayload, FailureKind,
    OperationFailure, Options, Progress, Reason, Severity, SourceIdentity, SourceOutcome,
    SourceSnapshot,
};

use instar_syntax::SyntaxSettings;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use super::settings::{
    CheckingMode, DiagnosticPolicy, OperationOverrides, ProjectSettings, ProjectSettingsPatch,
    canonical_flags,
};

use crate::{
    cache::{SettingsEntry, SettingsKey},
    capture::Capture,
    environments::diagnostic,
    execution::{Runtime, Work, WorkOutput},
    inputs::{InputIdentity, InputVersion, SourceOrigin, absolute, normalize},
    request::Invocation,
    selection,
};

pub(crate) const CONFIGURATION_MEMORY: u64 = 64 * 1024 * 1024;
pub(crate) const TYPE_FUNCTION_MEMORY: u64 = 64 * 1024 * 1024;

/// One accepted configuration layer with exact source bytes and anchored values.
#[derive(Clone, Debug, Serialize)]
pub struct ConfigurationLayer {
    /// Defining source identity.
    pub identity: SourceIdentity,

    /// Exact defining source revision and contents.
    pub source: SourceSnapshot,

    /// Directory anchoring relative values.
    pub base: PathBuf,

    /// Complete validated patch.
    pub patch: ProjectSettingsPatch,

    /// Consulted layer input guards.
    pub inputs: Vec<InputVersion>,
}

/// Current load outcome, retaining accepted historical layers on failure.
#[derive(Clone, Debug)]
pub enum ConfigurationState {
    /// Configuration has not been requested.
    Unloaded,

    /// Configuration loading is in progress.
    Loading,

    /// All requested current layers were validated.
    Ready {
        /// Ordered accepted current layers.
        layers: Vec<Arc<ConfigurationLayer>>,
        /// Current effective settings.
        effective: Box<ProjectSettings>,
        /// Every consulted input guard.
        inputs: Vec<InputVersion>,
    },

    /// Current settings are unavailable; historical layers do not replace them.
    Failed {
        /// Exact source-attributed failures.
        diagnostics: Vec<Diagnostic>,
        /// Previously accepted layers retained only as history.
        accepted_history: Vec<Arc<ConfigurationLayer>>,
    },
}

/// Validated settings for one exact project source and context.
#[derive(Clone, Debug, Serialize)]
pub struct EffectiveSettings {
    /// Selected project context.
    pub context: ContextIdentity,

    /// Selected source identity.
    pub source: SourceIdentity,

    /// Complete effective settings.
    pub settings: ProjectSettings,

    /// Actual request reporting policy, absent for formatting.
    pub diagnostics: Option<DiagnosticPolicy>,

    /// Every consulted input guarding these settings.
    pub inputs: Vec<InputVersion>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct LanguageLayer {
    #[serde(rename = "languageMode")]
    mode: Option<CheckingMode>,

    globals: Option<Vec<String>>,
    aliases: BTreeMap<String, String>,
    lint: Option<BTreeMap<String, bool>>,

    #[serde(rename = "lintErrors")]
    lint_errors: Option<bool>,

    #[serde(rename = "typeErrors")]
    type_errors: Option<bool>,
}

pub(crate) struct LayerResult {
    pub(crate) state: ConfigurationState,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) completion: Completion,
    error: Option<io::Error>,
}

#[derive(Default)]
struct ConfigurationLoading {
    layers: Vec<Arc<ConfigurationLayer>>,
    diagnostics: Vec<Diagnostic>,
    consulted: Vec<PathBuf>,
}

struct ConfigurationLocation {
    context: ContextIdentity,
    directory: PathBuf,
    base: PathBuf,
}

struct SettingsAssembly {
    settings: ProjectSettings,
    precedence: usize,
}

impl SettingsAssembly {
    fn apply(
        &mut self,
        configuration: &ProjectSettingsPatch,
        base: &Path,
        options: &Options,
    ) -> io::Result<()> {
        configuration.apply(&mut self.settings, Some((base, options)))?;

        let patch = &configuration.linting;

        for definition in instar_lint::rules() {
            if patch.presets.is_some()
                || patch.selected.is_some()
                || patch
                    .rules
                    .as_ref()
                    .is_some_and(|rules| rules.contains_key(&definition.identity))
                || patch
                    .warnings
                    .as_ref()
                    .is_some_and(|warnings| warnings.contains_key(&definition.identity))
                || patch.upstream_errors.is_some()
                    && matches!(definition.producer, RuleProducer::Upstream(_))
            {
                self.settings
                    .lint_priorities
                    .insert(definition.identity, self.precedence);
            }
        }

        Ok(())
    }
}

impl Runtime {
    pub(crate) fn effective(
        &mut self,
        source: SourceIdentity,
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<EffectiveSettings> {
        let result = self.load(source, invocation, overrides, capture, options);

        for finding in &result.diagnostics {
            options.report(Event {
                invocation: 0,
                operation: 0,
                payload: EventPayload::Diagnostic(finding.clone()),
            });
        }

        if let Some(error) = result.error {
            let mut failure = error
                .get_ref()
                .and_then(|error| error.downcast_ref::<OperationFailure>())
                .cloned()
                .unwrap_or_else(|| OperationFailure {
                    kind: match error.kind() {
                        io::ErrorKind::WouldBlock => FailureKind::Busy,

                        io::ErrorKind::InvalidInput
                        | io::ErrorKind::InvalidData
                        | io::ErrorKind::NotFound => FailureKind::InvalidInvocation,

                        _ => FailureKind::Operational,
                    },
                    message: error.to_string(),
                    completion: result.completion.clone(),
                    diagnostics: Vec::new(),
                    completed_sources: Vec::new(),
                    completed_edits: Vec::new(),
                });

            failure.completion = failure.completion.combine(result.completion);

            for finding in result.diagnostics {
                if !failure
                    .diagnostics
                    .iter()
                    .any(|previous| previous.identity == finding.identity)
                {
                    failure.diagnostics.push(finding);
                }
            }

            return Err(io::Error::new(error.kind(), failure));
        }

        if result.completion != Completion::Complete {
            return Err(result.completion.error());
        }

        let ConfigurationState::Ready {
            effective, inputs, ..
        } = result.state
        else {
            return Err(io::Error::other(
                "configuration loading produced no accepted settings",
            ));
        };

        Ok(EffectiveSettings {
            context: capture.snapshot.context(source),
            source,
            settings: *effective,
            diagnostics: overrides.diagnostics().cloned(),
            inputs,
        })
    }

    fn load(
        &mut self,
        source: SourceIdentity,
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        options: &Options,
    ) -> LayerResult {
        let mut loading = ConfigurationLoading::default();
        capture.begin_consultation();

        let outcome = self.resolve_settings(
            source,
            invocation,
            overrides,
            capture,
            &mut loading,
            options,
        );

        let inputs = capture.end_consultation();

        match outcome {
            Ok(settings) => LayerResult {
                state: ConfigurationState::Ready {
                    layers: loading.layers,
                    effective: Box::new(settings),
                    inputs,
                },
                diagnostics: loading.diagnostics,
                completion: Completion::Complete,
                error: None,
            },

            Err(error) => self.failed_settings(source, capture, loading, error, options),
        }
    }

    fn failed_settings(
        &self,
        source: SourceIdentity,
        capture: &Capture,
        mut loading: ConfigurationLoading,
        error: io::Error,
        options: &Options,
    ) -> LayerResult {
        if let Some(failure) = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<OperationFailure>())
        {
            for finding in &failure.diagnostics {
                if !loading
                    .diagnostics
                    .iter()
                    .any(|previous| previous.identity == finding.identity)
                {
                    loading.diagnostics.push(finding.clone());
                }
            }
        }

        if loading.diagnostics.is_empty()
            && options.interrupted().is_none()
            && !matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
            )
            && let Some(source) = capture.snapshot.source(source)
        {
            loading.diagnostics.push(diagnostic(
                source,
                "configuration.invalid",
                DiagnosticCategory::Configuration,
                error.to_string(),
            ));
        }

        let completion = failure_completion(&error, options);
        let mut accepted_history: Vec<Arc<ConfigurationLayer>> = Vec::new();

        for filename in loading.consulted {
            if let Some(layer) = self.cache.history.get(&filename)
                && !accepted_history
                    .iter()
                    .any(|previous| previous.source.version == layer.source.version)
            {
                accepted_history.push(Arc::clone(layer));
            }
        }

        LayerResult {
            state: ConfigurationState::Failed {
                diagnostics: loading.diagnostics.clone(),
                accepted_history,
            },
            diagnostics: loading.diagnostics,
            completion,
            error: Some(error),
        }
    }

    fn resolve_settings(
        &mut self,
        source: SourceIdentity,
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        loading: &mut ConfigurationLoading,
        options: &Options,
    ) -> io::Result<ProjectSettings> {
        let location = configuration_location(source, invocation, capture, options)?;

        let key = SettingsKey {
            context: location.context,
            directory: location.directory.clone(),
            base: location.base.clone(),
            configuration: invocation
                .configuration
                .as_deref()
                .map(absolute)
                .transpose()?,
            overrides: serde_json::to_string(overrides).map_err(io::Error::other)?,
        };

        if let Some(entry) = self.cache.settings.get(&key).map(Arc::clone) {
            let paths = entry
                .inputs
                .iter()
                .filter_map(|input| match &input.identity {
                    InputIdentity::Path(path) => Some(path.clone()),

                    InputIdentity::Source(source)
                    | InputIdentity::Configuration(source)
                    | InputIdentity::Sourcemap(source) => capture
                        .snapshot
                        .origin(*source)
                        .and_then(SourceOrigin::path)
                        .map(Path::to_path_buf),

                    _ => None,
                })
                .collect::<BTreeSet<_>>();

            loading.consulted.clone_from(&entry.consulted);

            for path in paths {
                capture.read(&path, options)?;
            }

            for input in &entry.inputs {
                if let InputIdentity::Directory(path) = &input.identity {
                    capture.listing(path, options)?;
                }
            }

            if capture.snapshot.current(&entry.inputs) {
                for input in &entry.inputs {
                    capture.consult_input(input.clone());
                }

                options.check()?;
                loading.layers.clone_from(&entry.layers);

                return Ok(entry.settings.clone());
            }

            self.cache.settings.remove(&key);
        }

        capture.begin_consultation();

        if let Some(input) = capture
            .snapshot
            .input_versions
            .get(&InputIdentity::Context(location.context))
            .cloned()
        {
            capture.consult_input(input);
        }

        let outcome =
            self.assemble_settings(&location, invocation, overrides, capture, loading, options);

        let inputs = capture.end_consultation();
        let settings = outcome?;

        if loading.diagnostics.is_empty() {
            if self.cache.settings.len() >= 128 {
                self.cache.settings.pop_first();
            }

            self.cache.settings.insert(
                key,
                Arc::new(SettingsEntry {
                    settings: settings.clone(),
                    layers: loading.layers.clone(),
                    inputs,
                    consulted: loading.consulted.clone(),
                }),
            );
        }

        Ok(settings)
    }

    pub(crate) fn profile(
        &mut self,
        flags: &BTreeMap<String, FlagValue>,
    ) -> io::Result<LanguageProfile> {
        if let Some((_, profile)) = self
            .cache
            .profiles
            .iter()
            .find(|(selected, profile)| selected == flags || &profile.flags == flags)
        {
            return Ok(profile.clone());
        }

        let profile = LanguageProfile::new(flags, TYPE_FUNCTION_MEMORY)?;

        if self.cache.profiles.len() >= 32 {
            self.cache.profiles.remove(0);
        }

        self.cache.profiles.push((flags.clone(), profile.clone()));

        Ok(profile)
    }

    fn assemble_settings(
        &mut self,
        location: &ConfigurationLocation,
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        loading: &mut ConfigurationLoading,
        options: &Options,
    ) -> io::Result<ProjectSettings> {
        let baseline = self.profile(&BTreeMap::new())?.flags;

        let mut assembly = SettingsAssembly {
            settings: ProjectSettings::default(),
            precedence: 1,
        };

        if let OperationOverrides::Format(overrides) = overrides {
            overrides.defaults.apply(&mut assembly.settings.formatting);
            assembly.settings.formatting.validate()?;
        }

        assembly.settings.language.flags.clone_from(&baseline);

        for directory in configuration_directories(location, invocation)? {
            self.load_directory(
                &directory,
                invocation,
                capture,
                loading,
                &mut assembly,
                options,
            )?;
        }

        apply_client(capture, location, &mut assembly, options)?;
        apply_invocation(overrides, &location.base, &mut assembly, options)?;
        discover_defaults(location, &mut assembly, capture, options)?;

        assembly
            .settings
            .linting
            .globals
            .clone_from(&assembly.settings.environment.globals);

        assembly.settings.linting.roblox = assembly.settings.environment.roblox;
        assembly.settings.linting.validate()?;

        assembly.settings.language.flags = self.profile(&assembly.settings.language.flags)?.flags;

        assembly.settings.syntax = grammar(&assembly.settings.language, &baseline);
        options.check()?;

        Ok(assembly.settings)
    }

    fn load_directory(
        &mut self,
        directory: &Path,
        invocation: &Invocation,
        capture: &mut Capture,
        loading: &mut ConfigurationLoading,
        assembly: &mut SettingsAssembly,
        options: &Options,
    ) -> io::Result<()> {
        options.check()?;
        let declarative = directory.join("instar.toml");
        let executable = directory.join("instar.luau");
        let language = directory.join(".luaurc");

        loading
            .consulted
            .extend([language.clone(), declarative.clone(), executable.clone()]);

        let declarative_source = capture.read(&declarative, options)?;
        let executable_source = capture.read(&executable, options)?;

        if let (Some(declarative_source), Some(executable_source)) =
            (&declarative_source, &executable_source)
        {
            for source in [declarative_source, executable_source] {
                loading.diagnostics.push(diagnostic(
                    source,
                    "configuration.competing",
                    DiagnosticCategory::Configuration,
                    format!(
                        "{}: instar.toml and instar.luau are competing definitions",
                        directory.display()
                    ),
                ));
            }

            return Err(invalid(format!(
                "{}: instar.toml and instar.luau are competing definitions",
                directory.display()
            )));
        }

        if let Some(source) = capture.read(&language, options)? {
            self.load_configuration(&language, &source, capture, loading, assembly, options)?;
        }

        let selected = if let Some(explicit) = &invocation.configuration {
            let explicit = absolute(explicit)?;
            loading.consulted.push(explicit.clone());

            if explicit == language {
                capture.source(&explicit, options)?;

                None
            } else {
                Some((explicit.clone(), capture.source(&explicit, options)?))
            }
        } else {
            declarative_source
                .map(|source| (declarative, source))
                .or_else(|| executable_source.map(|source| (executable, source)))
        };

        if let Some((filename, source)) = selected {
            self.load_configuration(&filename, &source, capture, loading, assembly, options)?;
        }

        Ok(())
    }

    fn load_configuration(
        &mut self,
        filename: &Path,
        source: &SourceSnapshot,
        capture: &mut Capture,
        loading: &mut ConfigurationLoading,
        assembly: &mut SettingsAssembly,
        options: &Options,
    ) -> io::Result<()> {
        let layer = self.layer(
            filename,
            source,
            &assembly.settings.language,
            capture,
            &mut loading.diagnostics,
            options,
        )?;

        assembly.apply(&layer.patch, &layer.base, options)?;

        loading.layers.push(layer);
        assembly.precedence += 1;

        Ok(())
    }

    fn layer(
        &mut self,
        filename: &Path,
        source: &SourceSnapshot,
        language: &LanguageFeatures,
        capture: &mut Capture,
        diagnostics: &mut Vec<Diagnostic>,
        options: &Options,
    ) -> io::Result<Arc<ConfigurationLayer>> {
        options.check()?;
        let context = capture.snapshot.context(source.version.identity);

        let key = (
            source.version,
            filename.to_path_buf(),
            serde_json::to_string(&(language, context)).map_err(io::Error::other)?,
        );

        if let Some(layer) = self.cache.layers.get(&key)
            && capture.snapshot.current(&layer.inputs)
        {
            for input in &layer.inputs {
                capture.consult_input(input.clone());
            }

            return Ok(Arc::clone(layer));
        }

        let before = diagnostics.len();
        capture.begin_consultation();
        capture.consult_source(source);

        if let Some(input) = capture
            .snapshot
            .input_versions
            .get(&InputIdentity::Context(context))
            .cloned()
        {
            capture.consult_input(input);
        }

        let loaded = self.parsed_layer(filename, source, language, context, diagnostics, options);
        let inputs = capture.end_consultation();

        let layer = match loaded {
            Ok(mut layer) => {
                layer.inputs = inputs;

                Arc::new(layer)
            }

            Err(error) => {
                if diagnostics.len() == before
                    && options.interrupted().is_none()
                    && !matches!(
                        error.kind(),
                        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
                    )
                {
                    diagnostics.push(diagnostic(
                        source,
                        "configuration.invalid",
                        DiagnosticCategory::Configuration,
                        format!("{}: {error}", filename.display()),
                    ));
                }

                if let Some(failure) = error
                    .get_ref()
                    .and_then(|error| error.downcast_ref::<OperationFailure>())
                {
                    let mut failure = failure.clone();
                    failure.message = format!("{}: {}", filename.display(), failure.message);

                    return Err(io::Error::new(error.kind(), failure));
                }

                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", filename.display()),
                ));
            }
        };

        self.cache.layers.insert(key, Arc::clone(&layer));

        if self.host.snapshot().current(&layer.inputs) {
            self.cache
                .history
                .insert(filename.to_path_buf(), Arc::clone(&layer));
        }

        Ok(layer)
    }

    fn parsed_layer(
        &mut self,
        filename: &Path,
        source: &SourceSnapshot,
        language: &LanguageFeatures,
        context: ContextIdentity,
        diagnostics: &mut Vec<Diagnostic>,
        options: &Options,
    ) -> io::Result<ConfigurationLayer> {
        let base = filename
            .parent()
            .ok_or_else(|| invalid("configuration has no defining directory"))?
            .to_path_buf();

        let mut configuration = if filename
            .extension()
            .is_some_and(|extension| extension == "toml")
        {
            let deserializer = toml::Deserializer::parse(&source.text).map_err(invalid)?;

            serde_path_to_error::deserialize::<_, ProjectSettingsPatch>(deserializer)
                .map_err(invalid)?
        } else if filename.file_name().is_some_and(|name| name == ".luaurc")
            || filename
                .extension()
                .is_some_and(|extension| extension == "luau")
        {
            let input = ConfigurationInput {
                source: source.clone(),
                language: language.clone(),
                context: ConfigurationContext {
                    identity: context,
                    base: base.to_string_lossy().into_owned(),
                    executable: filename.file_name().is_some_and(|name| name != ".luaurc"),
                },
                limits: ConfigurationLimits {
                    memory_bytes: CONFIGURATION_MEMORY,
                    extraction_depth: 128,
                    extraction_entries: 100_000,
                },
            };

            let value = self.evaluated_configuration(input, diagnostics, options)?;

            if filename.file_name().is_some_and(|name| name == ".luaurc") {
                language_patch(value)?
            } else {
                decode(value)?
            }
        } else {
            return Err(invalid("configuration must be .luaurc, TOML, or Luau"));
        };

        configuration.validate()?;
        anchor(&base, &mut configuration)?;
        configuration.language.flags = canonical_flags(&configuration.language.flags)?;
        configuration.validate()?;
        options.check()?;

        Ok(ConfigurationLayer {
            identity: source.version.identity,
            source: source.clone(),
            base,
            patch: configuration,
            inputs: Vec::new(),
        })
    }

    fn evaluated_configuration(
        &mut self,
        input: ConfigurationInput,
        diagnostics: &mut Vec<Diagnostic>,
        options: &Options,
    ) -> io::Result<Value> {
        let before = diagnostics.len();
        let recovered = Arc::new(Mutex::new(Vec::<Diagnostic>::new()));
        let collector = Arc::clone(&recovered);
        let forwarding = options.observer.clone();
        let mut evaluation = options.child();

        evaluation.observer = Some(Arc::new(move |event| {
            if let EventPayload::Diagnostic(finding) = &event.payload {
                collector
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(finding.clone());
            } else if let Some(callback) = &forwarding {
                callback(event);
            }
        }));

        evaluation.report(Event {
            invocation: self.invocation,
            operation: self.invocation,
            payload: EventPayload::Progress(Progress {
                phase: "configuration".to_owned(),
                source: Some(input.source.version),
                completed: 0,
                total: None,
            }),
        });

        let outcome = self
            .workers
            .execute(Work::Configuration(input), &evaluation);

        diagnostics.extend(
            recovered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .drain(..),
        );

        let WorkOutput::Configuration(result) = outcome? else {
            return Err(io::Error::other(
                "configuration lane returned wrong outcome",
            ));
        };

        let validation = if result.values.is_none()
            && matches!(
                &result.completion,
                Completion::Incomplete(reasons)
                    if matches!(
                        reasons.as_slice(),
                        [Reason::UnavailableSemantics | Reason::FailedPrerequisite]
                    )
            ) {
            result
                .diagnostics
                .iter()
                .find(|finding| {
                    finding.category == DiagnosticCategory::Configuration
                        && finding.severity == Severity::Error
                        && finding.primary.source == result.source
                })
                .map(|finding| {
                    if let Some(name) = finding.message.strip_prefix("Invalid alias ") {
                        format!("invalid module alias name {name}")
                    } else {
                        finding.message.clone()
                    }
                })
        } else {
            None
        };

        for finding in result.diagnostics {
            if !diagnostics
                .iter()
                .any(|previous| previous.identity == finding.identity)
            {
                diagnostics.push(finding);
            }
        }

        if result.completion != Completion::Complete {
            if let Some(message) = validation {
                return Err(OperationFailure {
                    kind: FailureKind::InvalidInvocation,
                    message,
                    completion: result.completion.clone(),
                    diagnostics: diagnostics[before..].to_vec(),
                    completed_sources: vec![SourceOutcome {
                        source: result.source,
                        completion: result.completion,
                    }],
                    completed_edits: Vec::new(),
                }
                .into());
            }

            return Err(result.completion.error());
        }

        if diagnostics.len() != before {
            return Err(invalid("configuration evaluator reported invalid settings"));
        }

        generic(
            result
                .values
                .ok_or_else(|| invalid("configuration evaluator returned no values"))?,
            "",
        )
    }
}

fn configuration_location(
    source: SourceIdentity,
    invocation: &Invocation,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<ConfigurationLocation> {
    options.check()?;

    let selected = capture
        .snapshot
        .source(source)
        .cloned()
        .ok_or_else(|| invalid("settings source is not captured"))?;

    capture.consult_source(&selected);
    let context = capture.snapshot.context(source);

    if let Some(input) = capture
        .snapshot
        .input_versions
        .get(&InputIdentity::Context(context))
        .cloned()
    {
        capture.consult_input(input);
    }

    let source_path = capture
        .snapshot
        .origin(source)
        .and_then(SourceOrigin::path)
        .map(Path::to_path_buf);

    let root = invocation
        .project_root
        .as_deref()
        .map(absolute)
        .transpose()?
        .or_else(|| {
            source_path.as_ref().and_then(|source_path| {
                capture
                    .snapshot
                    .roots()
                    .filter(|root| source_path.starts_with(root))
                    .max_by_key(|root| root.components().count())
                    .cloned()
            })
        });

    let directory = source_path
        .as_ref()
        .and_then(|source_path| source_path.parent())
        .map(Path::to_path_buf)
        .or_else(|| root.clone())
        .map_or_else(current_dir, Ok)?;

    if root
        .as_ref()
        .is_some_and(|root| !directory.starts_with(root))
    {
        return Err(invalid("source is outside the selected project root"));
    }

    let base = root.unwrap_or_else(|| directory.clone());

    Ok(ConfigurationLocation {
        context,
        directory,
        base,
    })
}

fn configuration_directories(
    location: &ConfigurationLocation,
    invocation: &Invocation,
) -> io::Result<Vec<PathBuf>> {
    if let Some(configuration) = &invocation.configuration {
        Ok(vec![
            absolute(configuration)?
                .parent()
                .ok_or_else(|| invalid("configuration has no defining directory"))?
                .to_path_buf(),
        ])
    } else {
        let mut directories = location
            .directory
            .ancestors()
            .map(Path::to_path_buf)
            .collect::<Vec<_>>();

        directories.reverse();

        Ok(directories)
    }
}

fn apply_client(
    capture: &Capture,
    location: &ConfigurationLocation,
    assembly: &mut SettingsAssembly,
    options: &Options,
) -> io::Result<()> {
    if let Some(mut configuration) = capture
        .snapshot
        .contexts
        .clients
        .get(&location.context)
        .cloned()
    {
        let base = capture
            .snapshot
            .contexts
            .roots
            .iter()
            .find_map(|(root, identity)| (*identity == location.context).then_some(root.clone()))
            .unwrap_or_else(|| location.base.clone());

        anchor(&base, &mut configuration)?;
        assembly.apply(&configuration, &base, options)?;
    }

    Ok(())
}

fn apply_invocation(
    overrides: &OperationOverrides,
    base: &Path,
    assembly: &mut SettingsAssembly,
    options: &Options,
) -> io::Result<()> {
    let mut configuration = overrides.patch();
    configuration.validate()?;

    anchor(base, &mut configuration)?;
    assembly.precedence = usize::MAX;
    assembly.settings.checking_override = configuration.checking.mode.is_some();
    assembly.apply(&configuration, base, options)?;

    Ok(())
}

fn discover_defaults(
    location: &ConfigurationLocation,
    assembly: &mut SettingsAssembly,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<()> {
    if !assembly.settings.resolution.sourcemaps_explicit
        && (!assembly.settings.environment.roblox_explicit || assembly.settings.environment.roblox)
    {
        for ancestor in location.directory.ancestors() {
            options.check()?;
            let map = ancestor.join("sourcemap.json");

            if capture.read(&map, options)?.is_some() {
                assembly.settings.resolution.sourcemaps = vec![map];

                break;
            }
        }
    }

    if !assembly.settings.environment.roblox_explicit {
        assembly.settings.environment.roblox = !assembly.settings.resolution.sourcemaps.is_empty();

        assembly.settings.linting.roblox = assembly.settings.environment.roblox;
    }

    Ok(())
}

fn failure_completion(error: &io::Error, options: &Options) -> Completion {
    options
        .interrupted()
        .or_else(|| {
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<OperationFailure>())
                .map(|failure| failure.completion.clone())
        })
        .or_else(|| {
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<Completion>())
                .cloned()
        })
        .or_else(|| {
            error
                .get_ref()
                .and_then(|error| error.downcast_ref::<Reason>())
                .map(|reason| Completion::Incomplete(vec![*reason]))
        })
        .or_else(|| match error.kind() {
            io::ErrorKind::Interrupted => Some(Completion::Cancelled),
            io::ErrorKind::TimedOut => Some(Completion::TimedOut),
            _ => None,
        })
        .unwrap_or_else(|| Completion::Incomplete(vec![Reason::FailedPrerequisite]))
}

pub(crate) fn worker_options(options: &Options) -> Options {
    let mut worker = options.child();

    if let Some(observer) = options.observer.clone() {
        worker.observer = Some(Arc::new(move |event| {
            if !matches!(event.payload, EventPayload::Diagnostic(_)) {
                observer(event);
            }
        }));
    }

    worker
}

fn anchor(base: &Path, patch: &mut ProjectSettingsPatch) -> io::Result<()> {
    patch.validate()?;

    for selected in [
        &mut patch.selection.checking,
        &mut patch.selection.formatting,
        &mut patch.selection.linting,
        &mut patch.selection.index,
        &mut patch.selection.imports,
    ] {
        for patterns in [&mut selected.include, &mut selected.exclude]
            .into_iter()
            .flatten()
        {
            for pattern in patterns {
                *pattern = selection::anchor(base, pattern)?;
            }
        }
    }

    for patterns in [&mut patch.selection.include, &mut patch.selection.exclude]
        .into_iter()
        .flatten()
    {
        for pattern in patterns {
            *pattern = selection::anchor(base, pattern)?;
        }
    }

    for path in patch.resolution.aliases.values_mut().flatten() {
        if !path.to_string_lossy().starts_with('@') {
            *path = base.join(&*path);
        }
    }

    for path in patch
        .resolution
        .sourcemaps
        .iter_mut()
        .flatten()
        .chain(patch.environment.capabilities.iter_mut())
        .chain(patch.environment.asset_directory.iter_mut())
    {
        *path = normalize(&base.join(&*path));
    }

    for group in patch.environment.groups.iter_mut().flatten() {
        for path in group.definitions.iter_mut().chain(&mut group.documentation) {
            *path = normalize(&base.join(&*path));
        }
    }

    Ok(())
}

fn language_patch(value: Value) -> io::Result<ProjectSettingsPatch> {
    let layer: LanguageLayer = decode(value)?;
    let mut patch = ProjectSettingsPatch::default();
    patch.checking.mode = layer.mode;
    patch.checking.type_errors = layer.type_errors;
    patch.environment.globals = layer.globals;

    patch.resolution.aliases = layer
        .aliases
        .into_iter()
        .map(|(name, path)| (name, Some(PathBuf::from(path))))
        .collect();

    patch.linting.upstream_errors = layer.lint_errors;

    patch.linting.warnings = layer
        .lint
        .map(|warnings| {
            let mut selected = BTreeMap::new();

            if let Some(enabled) = warnings.get("*") {
                for definition in instar_lint::rules() {
                    if matches!(definition.producer, RuleProducer::Upstream(_)) {
                        selected.insert(definition.identity, *enabled);
                    }
                }
            }

            for (name, enabled) in warnings {
                if name == "*" {
                    continue;
                }

                let identity = RuleIdentity::named(&name)
                    .ok_or_else(|| invalid(format!("lint.{name}: unknown Luau warning")))?;

                let definition = instar_lint::rules()
                    .iter()
                    .find(|definition| definition.identity == identity)
                    .ok_or_else(|| invalid("warning has no definition"))?;

                if !matches!(definition.producer, RuleProducer::Upstream(_)) {
                    return Err(invalid(format!("lint.{name}: unknown Luau warning")));
                }

                selected.insert(identity, enabled);
            }

            Ok(selected)
        })
        .transpose()?;

    Ok(patch)
}

fn decode<Output: DeserializeOwned>(value: Value) -> io::Result<Output> {
    serde_path_to_error::deserialize(value).map_err(invalid)
}

fn grammar(language: &LanguageFeatures, baseline: &BTreeMap<String, FlagValue>) -> SyntaxSettings {
    let selected = [
        "DebugLuauNoInline",
        "DebugLuauUserDefinedClasses",
        "LuauSingleTypeOptionalPackReturnsAttributeParens",
        "DebugLuauParseExactTables",
        "LuauExperimentalIfLocalSyntax",
        "LuauIntegerType2",
        "LuauExportValueSyntax",
    ];

    let features = selected
        .into_iter()
        .filter_map(|name| {
            let value = language.flags.get(name)?;

            (baseline.get(name) != Some(value)).then(|| match value {
                FlagValue::Boolean(value) => format!("{name}={value}"),
                FlagValue::Integer(value) => format!("{name}={value}"),
            })
        })
        .collect();

    SyntaxSettings {
        features,
        declaration_syntax: language.declaration_syntax,
        ..SyntaxSettings::default()
    }
}

fn generic(value: ConfigurationValue, path: &str) -> io::Result<Value> {
    match value {
        ConfigurationValue::String(value) => Ok(Value::String(value)),

        ConfigurationValue::Number(value) => {
            if !value.is_finite() {
                return Err(invalid("configuration contains a nonfinite number"));
            }

            serde_json::from_str(&value.to_string()).map_err(invalid)
        }

        ConfigurationValue::Boolean(value) => Ok(Value::Bool(value)),

        ConfigurationValue::Table(entries) => {
            let array = entries.is_empty()
                && matches!(
                    path.rsplit('.').next().unwrap_or(""),
                    "include"
                        | "exclude"
                        | "globals"
                        | "groups"
                        | "definitions"
                        | "documentation"
                        | "sourcemaps"
                        | "presets"
                        | "selected"
                        | "patterns"
                );

            if array
                || !entries.is_empty()
                    && entries
                        .iter()
                        .all(|entry| matches!(entry.key, ConfigurationKey::Number(_)))
            {
                let mut values = BTreeMap::new();

                for entry in entries {
                    let ConfigurationKey::Number(key) = entry.key else {
                        return Err(invalid("mixed configuration table"));
                    };

                    if key.fract() != 0.0 || !(1.0..=100_000.0).contains(&key) {
                        return Err(invalid("invalid configuration array key"));
                    }

                    let index = key.to_string().parse::<usize>().map_err(invalid)?;

                    if values
                        .insert(index, generic(entry.value, &format!("{path}[{index}]"))?)
                        .is_some()
                    {
                        return Err(invalid("duplicate configuration array key"));
                    }
                }

                if values.keys().copied().ne(1..=values.len()) {
                    return Err(invalid("configuration array keys must be contiguous"));
                }

                Ok(Value::Array(values.into_values().collect()))
            } else {
                let mut values = serde_json::Map::new();

                for entry in entries {
                    let ConfigurationKey::String(key) = entry.key else {
                        return Err(invalid("configuration object keys must be strings"));
                    };

                    let field = if path.is_empty() {
                        key.clone()
                    } else {
                        format!("{path}.{key}")
                    };

                    if values.insert(key, generic(entry.value, &field)?).is_some() {
                        return Err(invalid("duplicate configuration object key"));
                    }
                }

                Ok(Value::Object(values))
            }
        }
    }
}

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
