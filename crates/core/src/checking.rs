//! Native checking over the shared project source and dependency graph.

use crate::{
    configuration::{Security, invalid},
    native,
    project::{Project, absolute},
    resolve::{Identity, Module},
    source::Document,
};

use instar_bridge::frontend::Definition;
use instar_check::{Completion, Diagnostic, Kind, Location, Options, Reason, Related};

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    time::Instant,
};

/// An explicit checker entry with an optional exact sourcemap placement.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Absolute backing source path.
    pub source: PathBuf,

    /// Explicit identity when the source has multiple mapped placements.
    pub context: Option<Identity>,
}

impl Entry {
    /// Creates a source entry without choosing a mapped placement.
    #[must_use]
    pub fn new(source: PathBuf) -> Self {
        Self {
            source,
            context: None,
        }
    }
}

/// The exact source owner of a checker diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A module in the Rust-owned graph, retaining its contextual identity.
    Module(Module),

    /// An environment declaration file.
    Definition(PathBuf),
}

impl Origin {
    /// Returns the actual backing source path.
    #[must_use]
    pub fn source(&self) -> &Path {
        match self {
            Self::Module(module) => &module.source,
            Self::Definition(path) => path,
        }
    }
}

struct Resolution {
    location: Location<Origin>,
    call: [usize; 2],
    message: String,
}

#[derive(Default)]
struct Analysis {
    sources: BTreeMap<String, (Origin, Document)>,
    environments: BTreeMap<String, Vec<Definition>>,
    failures: Vec<Resolution>,
}

impl Analysis {
    fn report(
        self,
        checked: instar_check::Result<String>,
        result: &mut instar_check::Result<Origin>,
    ) -> io::Result<()> {
        result.completion = checked.completion;
        let mut resolution_codes = BTreeMap::new();

        for diagnostic in checked.diagnostics {
            let location = locate(&diagnostic.location, &self.sources)?;

            if matches!(diagnostic.kind, Kind::Resolution { .. })
                && let Some((index, _)) = self.failures.iter().enumerate().find(|(_, failure)| {
                    failure.location.module == location.module
                        && failure.location.revision == location.revision
                        && location.range[0] <= failure.call[1]
                        && failure.call[0] <= location.range[1]
                })
            {
                resolution_codes.insert(index, diagnostic.kind.native_code());
                continue;
            }

            let related = diagnostic
                .related
                .into_iter()
                .map(|related| {
                    Ok(Related {
                        location: locate(&related.location, &self.sources)?,
                        message: related.message,
                    })
                })
                .collect::<io::Result<Vec<_>>>()?;

            let diagnostic = Diagnostic {
                location,
                kind: diagnostic.kind,
                message: diagnostic.message,
                related,
            };

            if !result.diagnostics.contains(&diagnostic) {
                result.diagnostics.push(diagnostic);
            }
        }

        for (index, failure) in self.failures.into_iter().enumerate() {
            result.diagnostics.push(Diagnostic {
                location: failure.location,
                kind: Kind::Resolution {
                    code: resolution_codes.remove(&index).flatten(),
                },
                message: failure.message,
                related: Vec::new(),
            });
        }

        Ok(())
    }
}

fn selected(source: &Path, patterns: Option<&[String]>, empty: bool) -> io::Result<bool> {
    let Some(patterns) = patterns else {
        return Ok(empty);
    };

    if patterns.is_empty() {
        return Ok(empty);
    }

    for pattern in patterns {
        if glob::Pattern::new(pattern)
            .map_err(invalid)?
            .matches_path(source)
        {
            return Ok(true);
        }
    }

    Ok(false)
}

fn interrupted(options: &Options, started: Instant) -> Option<Reason> {
    if options.cancellation.requested() {
        Some(Reason::Cancelled)
    } else if started.elapsed() >= options.timeout {
        Some(Reason::Timeout)
    } else {
        None
    }
}

fn locate(
    location: &Location<String>,
    sources: &BTreeMap<String, (Origin, Document)>,
) -> io::Result<Location<Origin>> {
    let (origin, document) = sources
        .get(&location.module)
        .ok_or_else(|| invalid("native diagnostic has an unknown host identity"))?;

    if document.revision != location.revision
        || location.range[0] > location.range[1]
        || location.range[1] > document.text.len()
        || !document.text.is_char_boundary(location.range[0])
        || !document.text.is_char_boundary(location.range[1])
    {
        return Err(invalid(
            "native diagnostic is not anchored to its host revision",
        ));
    }

    Ok(Location {
        module: origin.clone(),
        revision: location.revision,
        range: location.range,
    })
}

impl Project {
    /// Checks selected entries and reports dependency errors once per exact module.
    ///
    /// Instar patterns select entries only. Native configuration controls language
    /// modes and type error visibility; native lint warnings are never emitted.
    /// Enabled Roblox checking requires explicit native declarations; Roblox API
    /// security filtering is unsupported. Native module analysis is cooperatively
    /// interrupted; host discovery, parsing and upstream declaration loading are
    /// budget-checked between phases.
    /// Missing declaration files use revision zero and an empty source range.
    ///
    /// # Errors
    /// Returns source/configuration failures and invalid host/native source agreement.
    pub fn check(
        &mut self,
        entries: &[Entry],
        options: &Options,
    ) -> io::Result<instar_check::Result<Origin>> {
        let started = Instant::now();

        let mut result = instar_check::Result {
            modules: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let mut roots = BTreeSet::new();
        let mut identities = BTreeSet::new();

        for entry in entries {
            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(result);
            }

            let source = absolute(&entry.source)?;

            let directory = source
                .parent()
                .ok_or_else(|| invalid("checker entry has no directory"))?;

            let settings = self.configuration(directory)?;

            if !selected(
                &source,
                settings.configuration.check.include.as_deref(),
                true,
            )? || selected(
                &source,
                settings.configuration.check.exclude.as_deref(),
                false,
            )? {
                continue;
            }

            let module = self.install(&source, entry.context.as_ref())?;
            roots.insert(native::name(&module.identity));
            identities.extend(self.discover(&source, entry.context.as_ref())?);
        }

        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);

            return Ok(result);
        }

        if roots.is_empty() {
            return Ok(result);
        }

        let mut analysis = Analysis::default();

        for identity in identities {
            self.check_module(&identity, options, started, &mut analysis, &mut result)?;

            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(result);
            }
        }

        if result.completion != Completion::Complete {
            return Ok(result);
        }

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        for (name, definitions) in &analysis.environments {
            frontend.definitions(name, definitions)?;
        }

        let remaining = Options {
            timeout: options.timeout.saturating_sub(started.elapsed()),
            cancellation: options.cancellation.clone(),
        };

        let checked = frontend.check(&roots.into_iter().collect::<Vec<_>>(), &remaining)?;
        analysis.report(checked, &mut result)?;

        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);
        }

        Ok(result)
    }

    fn check_module(
        &mut self,
        identity: &Identity,
        options: &Options,
        started: Instant,
        analysis: &mut Analysis,
        result: &mut instar_check::Result<Origin>,
    ) -> io::Result<()> {
        let node = self
            .graph
            .node(identity)
            .ok_or_else(|| invalid("checker module is missing from the Rust graph"))?;

        let origin = Origin::Module(node.module.clone());
        result.modules.push(origin.clone());
        let name = native::name(identity);

        analysis
            .sources
            .insert(name.clone(), (origin.clone(), node.document.clone()));

        let directory = node
            .module
            .source
            .parent()
            .ok_or_else(|| invalid("module source has no directory"))?;

        let settings = self.configuration(directory)?;

        if settings.configuration.roblox.enabled == Some(true)
            && (settings.configuration.environment.definitions.is_empty()
                || !matches!(settings.configuration.roblox.security, Security::None))
        {
            result.diagnostics.push(Diagnostic {
                location: Location { module: origin.clone(), revision: node.document.revision, range: [0, 0] },
                kind: Kind::Unsupported,
                message: "Roblox checking requires explicit native API declarations and does not support API security filtering".to_owned(),
                related: Vec::new(),
            });

            result.completion = Completion::Incomplete(Reason::Unsupported);
        }

        let mut definitions = Vec::new();

        for path in &settings.configuration.environment.definitions {
            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(());
            }

            let document = match self.source(path) {
                Ok(document) => document,

                Err(error) => {
                    let diagnostic = Diagnostic {
                        location: Location {
                            module: Origin::Definition(path.clone()),
                            revision: 0,
                            range: [0, 0],
                        },
                        kind: Kind::Analysis { code: None },
                        message: error.to_string(),
                        related: Vec::new(),
                    };

                    if !result.diagnostics.contains(&diagnostic) {
                        result.diagnostics.push(diagnostic);
                    }

                    result.completion = Completion::Incomplete(Reason::Environment);
                    continue;
                }
            };

            let definition = format!("Definition({})", path.display());

            analysis.sources.insert(
                definition.clone(),
                (Origin::Definition(path.clone()), document.clone()),
            );

            if !definitions
                .iter()
                .any(|existing: &Definition| existing.name == definition)
            {
                definitions.push(Definition {
                    name: definition,
                    revision: document.revision,
                    text: document.text.to_string(),
                });
            }
        }

        analysis.environments.insert(name, definitions);

        for site in &node.sites {
            if let Some(failure) = &site.failure {
                analysis.failures.push(Resolution {
                    location: Location {
                        module: origin.clone(),
                        revision: node.document.revision,
                        range: site.range,
                    },
                    call: site.call,
                    message: failure.message.clone(),
                });
            }
        }

        Ok(())
    }
}
