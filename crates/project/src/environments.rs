use std::{io, sync::Arc};

use instar_luau::{
    Capabilities, DeclarationGroup, Definition, EnvironmentIdentity, EnvironmentInput,
};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, FailureKind, FindingIdentity, Location,
    OperationFailure, Options, Reason, RelatedLocation, Severity, SourceSnapshot, Span,
};

use crate::{
    assets::{self, Assets},
    capture::Capture,
    configuration::EffectiveSettings,
    documentation::Index,
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
    fn failure(&mut self, error: io::Error, source: &SourceSnapshot) -> io::Result<()> {
        if matches!(
            error.kind(),
            io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
        ) {
            return Err(error);
        }

        let finding = if let Some(failure) = error
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

            unavailable(source, &failure.diagnostics, error.to_string())
        } else {
            let completion = Completion::Incomplete(vec![Reason::UnavailableEnvironment]);

            self.completion = self.completion.clone().combine(completion.clone());

            if error.kind() != io::ErrorKind::NotFound {
                self.failures.push(OperationFailure {
                    kind: FailureKind::Operational,
                    message: error.to_string(),
                    completion,
                    diagnostics: Vec::new(),
                    completed_sources: Vec::new(),
                    completed_edits: Vec::new(),
                });
            }

            unavailable(source, &[], error.to_string())
        };

        if let Some(previous) = self.diagnostics.iter_mut().find(|diagnostic| {
            diagnostic.code == "environment.unavailable"
                && diagnostic.primary.source == source.version
        }) {
            previous.message.push_str("; ");
            previous.message.push_str(&finding.message);
            previous.related.extend(finding.related);

            previous.related.sort_by(|left, right| {
                left.location
                    .cmp(&right.location)
                    .then_with(|| left.message.cmp(&right.message))
            });

            previous.related.dedup();

            previous.identity = FindingIdentity::new(
                "project",
                previous
                    .identity
                    .inputs()
                    .iter()
                    .chain(finding.identity.inputs())
                    .copied()
                    .collect(),
                &previous.code,
                previous.primary,
            );
        } else {
            self.diagnostics.push(finding);
        }

        Ok(())
    }
}

impl Runtime {
    pub(crate) fn environment(
        &self,
        effective: &EffectiveSettings,
        map: Option<&(Arc<Map>, usize)>,
        capture: &mut Capture,
        requested: bool,
        members: bool,
        options: &Options,
    ) -> io::Result<EnvironmentRecipe> {
        let settings = &effective.settings;

        let source = capture
            .snapshot
            .source(effective.source)
            .cloned()
            .ok_or_else(|| io::Error::other("environment source is absent from its capture"))?;

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

        if members || requested && settings.environment.roblox {
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
                    if requested && settings.environment.roblox {
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

                Err(error) => recipe.failure(error, &source)?,
            }
        }

        if !requested {
            return Ok(recipe);
        }

        if let Some(path) = &settings.environment.capabilities {
            match capture.source(path, options) {
                Ok(input) => match serde_json::from_str(&input.text)
                    .map_err(io::Error::other)
                    .map_err(|error| {
                        assets::attributed(&input, &[], "environment.capabilities", error)
                    }) {
                    Ok(capabilities) => recipe.input.capabilities = capabilities,
                    Err(error) => recipe.failure(error, &source)?,
                },

                Err(error) => recipe.failure(error, &source)?,
            }
        }

        let mut documentation = Index::default();

        for group in &settings.environment.groups {
            options.check()?;
            let mut definitions = Vec::new();

            for path in &group.definitions {
                match capture.source(path, options) {
                    Ok(source) => definitions.push(Definition {
                        source,
                        namespace: group.namespace.clone(),
                    }),

                    Err(error) => recipe.failure(error, &source)?,
                }
            }

            for path in &group.documentation {
                match capture.source(path, options) {
                    Ok(input) => {
                        let parsed = Index::parse(&input.text, &[&group.namespace])
                            .and_then(|index| documentation.merge(index))
                            .map_err(|error| {
                                assets::attributed(&input, &[], "environment.documentation", error)
                            });

                        if let Err(error) = parsed {
                            recipe.failure(error, &source)?;
                        }
                    }

                    Err(error) => recipe.failure(error, &source)?,
                }
            }

            recipe
                .input
                .declarations
                .push(DeclarationGroup { definitions });
        }

        for definition in recipe
            .input
            .declarations
            .iter()
            .flat_map(|group| &group.definitions)
        {
            capture.declaration(&definition.source)?;
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

pub(crate) fn unavailable(
    source: &SourceSnapshot,
    prerequisites: &[Diagnostic],
    message: String,
) -> Diagnostic {
    let mut finding = diagnostic(
        source,
        "environment.unavailable",
        DiagnosticCategory::Analysis,
        message,
    );

    finding.related = prerequisites
        .iter()
        .flat_map(|diagnostic| {
            let primary = RelatedLocation {
                location: diagnostic.primary,
                message: diagnostic.message.clone(),
            };

            [primary]
                .into_iter()
                .chain(diagnostic.related.iter().cloned())
        })
        .collect();

    finding.related.sort_by(|left, right| {
        left.location
            .cmp(&right.location)
            .then_with(|| left.message.cmp(&right.message))
    });

    finding.related.dedup();

    finding.identity = FindingIdentity::new(
        "project",
        prerequisites
            .iter()
            .flat_map(|diagnostic| {
                diagnostic
                    .identity
                    .inputs()
                    .iter()
                    .copied()
                    .chain([diagnostic.primary.source])
                    .chain(
                        diagnostic
                            .related
                            .iter()
                            .map(|related| related.location.source),
                    )
            })
            .chain([source.version])
            .collect(),
        &finding.code,
        finding.primary,
    );

    finding
}
