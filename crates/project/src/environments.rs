use std::{io, sync::Arc};

use instar_luau::{
    Capabilities, DeclarationGroup, Definition, EnvironmentIdentity, EnvironmentInput,
};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, FailureKind, FindingIdentity, Location,
    OperationFailure, Options, Reason, Severity, SourceSnapshot, Span,
};

use crate::{
    assets::{self, Assets},
    capture::Capture,
    configuration::EffectiveSettings,
    execution::Runtime,
    inputs::InputVersion,
    sourcemap::Map,
};

pub(crate) struct EnvironmentRecipe {
    pub(crate) input: EnvironmentInput,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) completion: Completion,
    pub(crate) assets: Option<Assets>,
    pub(crate) requested: bool,
    pub(crate) member_inputs: Vec<InputVersion>,
    pub(crate) failures: Vec<OperationFailure>,
}

impl EnvironmentRecipe {
    fn failure(&mut self, error: io::Error) -> io::Result<()> {
        if matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
        ) {
            return Err(error);
        }

        if let Some(failure) = error
            .get_ref()
            .and_then(|error| error.downcast_ref::<OperationFailure>())
        {
            if matches!(
                failure.completion,
                Completion::Cancelled | Completion::TimedOut
            ) {
                return Err(error);
            }

            self.diagnostics.extend(failure.diagnostics.clone());
            self.completion = self.completion.clone().combine(failure.completion.clone());

            if failure.kind == FailureKind::Operational || failure.diagnostics.is_empty() {
                self.failures.push(failure.clone());
            }
        } else {
            let completion = Completion::Incomplete(vec![Reason::UnavailableEnvironment]);
            self.completion = self.completion.clone().combine(completion.clone());

            self.failures.push(OperationFailure {
                kind: FailureKind::Operational,
                message: error.to_string(),
                completion,
                diagnostics: Vec::new(),
                completed_sources: Vec::new(),
                completed_edits: Vec::new(),
            });
        }

        Ok(())
    }
}

impl Runtime {
    pub(crate) fn environment(
        &mut self,
        effective: &EffectiveSettings,
        map: Option<&(Arc<Map>, usize)>,
        capture: &mut Capture,
        requested: bool,
        members: bool,
        options: &Options,
    ) -> io::Result<EnvironmentRecipe> {
        let settings = &effective.settings;

        let mut recipe = EnvironmentRecipe {
            input: EnvironmentInput {
                identity: EnvironmentIdentity::new(self.identity()),
                language: settings.language.clone(),
                globals: settings.environment.globals.clone(),
                declarations: Vec::new(),
                capabilities: Capabilities::default(),
                instances: map.map(|(map, script)| map.environment(*script)),
            },
            diagnostics: Vec::new(),
            completion: Completion::Complete,
            assets: None,
            requested,
            member_inputs: Vec::new(),
            failures: Vec::new(),
        };

        if (requested || members) && settings.environment.roblox {
            capture.begin_consultation();

            let loaded = settings
                .environment
                .asset_directory
                .clone()
                .map_or_else(assets::roblox_cache_directory, Ok)
                .and_then(|directory| {
                    assets::load(&directory, settings.environment.security, capture, options)
                });

            recipe.member_inputs = capture.end_consultation();

            match loaded {
                Ok(assets) => {
                    if requested {
                        recipe.input.declarations.push(DeclarationGroup {
                            definitions: assets
                                .definitions
                                .iter()
                                .map(|source| Definition {
                                    source: source.clone(),
                                    namespace: "@roblox".to_owned(),
                                })
                                .collect(),
                        });

                        recipe.input.capabilities = assets.capabilities.clone();
                    }

                    recipe.assets = Some(assets);
                }

                Err(error) => recipe.failure(error)?,
            }
        }

        if !requested {
            return Ok(recipe);
        }

        if let Some(path) = &settings.environment.capabilities {
            match capture.source(path, options) {
                Ok(source) => match serde_json::from_str(&source.text)
                    .map_err(io::Error::other)
                    .map_err(|error| {
                        assets::attributed(&source, &[], "environment.capabilities", error)
                    }) {
                    Ok(capabilities) => recipe.input.capabilities = capabilities,
                    Err(error) => recipe.failure(error)?,
                },

                Err(error) => recipe.failure(error)?,
            }
        }

        for group in &settings.environment.groups {
            options.check()?;
            let mut definitions = Vec::new();

            for path in &group.definitions {
                match capture.source(path, options) {
                    Ok(source) => definitions.push(Definition {
                        source,
                        namespace: group.namespace.clone(),
                    }),

                    Err(error) => recipe.failure(error)?,
                }
            }

            recipe
                .input
                .declarations
                .push(DeclarationGroup { definitions });
        }

        Ok(recipe)
    }
}

pub(crate) fn diagnostic(
    source: &SourceSnapshot,
    code: &str,
    category: DiagnosticCategory,
    message: String,
) -> Diagnostic {
    let primary = Location {
        source: source.version,
        span: Span { start: 0, end: 0 },
    };

    Diagnostic {
        identity: FindingIdentity::new("project", vec![source.version], code, primary),
        code: code.to_owned(),
        category,
        severity: Severity::Error,
        message,
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    }
}
