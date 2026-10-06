//! Lint orchestration over shared sources, contextual resolution and native analysis.

pub use crate::checking::{Entry, Origin};

use crate::{
    configuration::{Security, invalid},
    native,
    project::{Project, Settings, absolute},
    resolve::Request,
    source::Document,
};

use instar_bridge::frontend::{Definition, FactKind, LintResult};

use instar_lint::{
    Completion, Diagnostic, Inference, Kind, Level, Location, Options, Reason, Related, Require,
    Source,
};

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    rc::Rc,
    time::Instant,
};

struct Module {
    node: Rc<crate::graph::Node>,
    settings: Rc<Settings>,
    selected: bool,
}

#[derive(Default)]
struct Analysis {
    roots: BTreeMap<String, crate::resolve::Module>,
    sources: BTreeMap<String, (Origin, Document)>,
    modules: BTreeMap<String, Module>,
    definitions: BTreeMap<String, Vec<Definition>>,
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

fn eligible(source: &Path, settings: &Settings) -> io::Result<bool> {
    let lint = &settings.configuration.lint;

    Ok(selected(source, lint.include.as_deref(), true)?
        && !selected(source, lint.exclude.as_deref(), false)?)
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

fn remaining(options: &Options, started: Instant) -> Options {
    Options {
        timeout: options.timeout.saturating_sub(started.elapsed()),
        cancellation: options.cancellation.clone(),
    }
}

fn locate(
    location: &Location<String>,
    sources: &BTreeMap<String, (Origin, Document)>,
) -> io::Result<Location<Origin>> {
    let (origin, document) = sources
        .get(&location.module)
        .ok_or_else(|| invalid("native lint diagnostic has an unknown host identity"))?;

    if location.revision != document.revision
        || location.range[0] > location.range[1]
        || location.range[1] > document.text.len()
        || !document.text.is_char_boundary(location.range[0])
        || !document.text.is_char_boundary(location.range[1])
    {
        return Err(invalid(
            "native lint diagnostic is not anchored to its host revision",
        ));
    }

    Ok(Location {
        module: origin.clone(),
        revision: document.revision,
        range: location.range,
    })
}

fn error(
    result: &mut instar_lint::Result<Origin>,
    location: Location<Origin>,
    kind: Kind,
    message: String,
) {
    let diagnostic = Diagnostic {
        location,
        kind,
        level: Level::Deny,
        message,
        related: Vec::new(),
    };

    if !result.diagnostics.contains(&diagnostic) {
        result.diagnostics.push(diagnostic);
    }
}

impl Project {
    /// Lints selected contextual sources using independent native and Instar policies.
    ///
    /// Include/exclude settings select root entries and lint findings in dependencies;
    /// dependency analysis errors are never filtered. Native warnings use the shared
    /// native frontend and declaration environments. Instar syntax rules do not load
    /// declarations when no native warning or inferred-type rule is requested.
    /// Roblox API declarations and security filtering are explicit capability limits.
    /// Native parsing and declaration loading are budget-checked between phases;
    /// module analysis and syntax traversal support cooperative cancellation.
    /// No fixes are executed.
    ///
    /// # Errors
    /// Returns host source/configuration failures or invalid host/native agreement.
    pub fn lint(
        &mut self,
        entries: &[Entry],
        options: &Options,
    ) -> io::Result<instar_lint::Result<Origin>> {
        let started = Instant::now();

        let mut result = instar_lint::Result {
            modules: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let mut analysis = Analysis::default();
        self.lint_sources(entries, &mut analysis, &mut result, options, started)?;

        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);

            return Ok(result);
        }

        let semantic = analysis
            .modules
            .iter()
            .filter(|(_, module)| module.selected && module.settings.configuration.lint.semantic())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        let native_requested = !semantic.is_empty()
            || analysis.modules.values().any(|module| {
                module.selected
                    && module
                        .settings
                        .snapshot
                        .lint
                        .values()
                        .any(|policy| policy.enabled)
            });

        let facts = if native_requested && !analysis.roots.is_empty() {
            self.lint_native(&mut analysis, &mut result, &semantic, options, started)?
        } else {
            BTreeMap::new()
        };

        analysis.syntax(&facts, &mut result, native_requested, options, started)?;

        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);
        }

        Ok(result)
    }

    fn lint_sources(
        &mut self,
        entries: &[Entry],
        analysis: &mut Analysis,
        result: &mut instar_lint::Result<Origin>,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        let mut identities = BTreeSet::new();

        for entry in entries {
            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(());
            }

            let source = absolute(&entry.source)?;

            let directory = source
                .parent()
                .ok_or_else(|| invalid("lint entry has no directory"))?;

            let settings = self.configuration(directory)?;

            if !eligible(&source, &settings)? {
                continue;
            }

            let node = self.links(&source, entry.context.as_ref())?;

            analysis
                .roots
                .insert(native::name(&node.module.identity), node.module.clone());

            identities.extend(self.discover(&source, entry.context.as_ref())?);
        }

        for identity in identities {
            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(());
            }

            self.lint_module(&identity, analysis, result)?;
        }

        Ok(())
    }

    fn lint_module(
        &mut self,
        identity: &crate::resolve::Identity,
        analysis: &mut Analysis,
        result: &mut instar_lint::Result<Origin>,
    ) -> io::Result<()> {
        let node = self
            .graph
            .node(identity)
            .ok_or_else(|| invalid("lint module is missing from the shared graph"))?;

        let directory = node
            .module
            .source
            .parent()
            .ok_or_else(|| invalid("lint module has no directory"))?;

        let settings = self.configuration(directory)?;
        let selected = eligible(&node.module.source, &settings)?;
        let name = native::name(identity);
        let origin = Origin::Module(node.module.clone());

        if selected {
            result.modules.push(origin.clone());
        }

        for site in &node.sites {
            if let Some(failure) = &site.failure {
                error(
                    result,
                    Location {
                        module: origin.clone(),
                        revision: node.document.revision,
                        range: site.range,
                    },
                    Kind::Resolution,
                    failure.message.clone(),
                );
            }
        }

        if !selected {
            for problem in &node.problems {
                error(
                    result,
                    Location {
                        module: origin.clone(),
                        revision: node.document.revision,
                        range: problem.range,
                    },
                    Kind::Syntax,
                    problem.message.clone(),
                );
            }
        }

        analysis
            .sources
            .insert(name.clone(), (origin, node.document.clone()));

        analysis.modules.insert(
            name,
            Module {
                node,
                settings,
                selected,
            },
        );

        Ok(())
    }

    fn lint_native(
        &mut self,
        analysis: &mut Analysis,
        result: &mut instar_lint::Result<Origin>,
        semantic: &[String],
        options: &Options,
        started: Instant,
    ) -> io::Result<BTreeMap<String, Vec<Inference>>> {
        let mut facts = BTreeMap::new();

        for module in analysis.roots.values() {
            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(facts);
            }

            self.install(&module.source, Some(&module.identity))?;
        }

        self.lint_definitions(analysis, result, options, started);

        if let Some(reason) = interrupted(options, started) {
            result.completion = Completion::Incomplete(reason);

            return Ok(facts);
        }

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        for (name, definitions) in &analysis.definitions {
            frontend.definitions(name, definitions)?;
        }

        let roots = analysis.roots.keys().cloned().collect::<Vec<_>>();
        let limits = remaining(options, started);

        let native = if semantic.is_empty() {
            frontend.lint(&roots, &limits)?
        } else {
            frontend.lint_semantic(&roots, semantic, &limits)?
        };

        analysis.native(native, &mut facts, result)?;

        Ok(facts)
    }

    fn lint_definitions(
        &mut self,
        analysis: &mut Analysis,
        result: &mut instar_lint::Result<Origin>,
        options: &Options,
        started: Instant,
    ) {
        for (name, module) in &analysis.modules {
            let settings = &module.settings.configuration;

            if settings.roblox.enabled == Some(true)
                && (settings.environment.definitions.is_empty()
                    || !matches!(settings.roblox.security, Security::None))
            {
                error(result, Location { module: Origin::Module(module.node.module.clone()), revision: module.node.document.revision, range: [0, 0] }, Kind::Unsupported, "Native Roblox lint analysis requires explicit API declarations and does not support API security filtering".to_owned());
                result.completion = Completion::Incomplete(Reason::Unsupported);
            }

            let mut definitions = Vec::new();

            for path in &settings.environment.definitions {
                if let Some(reason) = interrupted(options, started) {
                    result.completion = Completion::Incomplete(reason);

                    return;
                }

                match self.source(path) {
                    Ok(document) => {
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

                    Err(failure) => {
                        error(
                            result,
                            Location {
                                module: Origin::Definition(path.clone()),
                                revision: 0,
                                range: [0, 0],
                            },
                            Kind::Analysis,
                            failure.to_string(),
                        );

                        result.completion = Completion::Incomplete(Reason::Environment);
                    }
                }
            }

            analysis.definitions.insert(name.clone(), definitions);
        }
    }
}

impl Analysis {
    fn syntax(
        &self,
        facts: &BTreeMap<String, Vec<Inference>>,
        result: &mut instar_lint::Result<Origin>,
        native_requested: bool,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        for (name, module) in &self.modules {
            if !module.selected {
                continue;
            }

            if let Some(reason) = interrupted(options, started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(());
            }

            let requires = module
                .node
                .sites
                .iter()
                .map(|site| Require {
                    call: site.call,
                    argument: site.range,
                    constant: site.request.is_some(),
                    path: match &site.request {
                        Some(Request::String(path)) => Some(path.clone()),
                        _ => None,
                    },
                })
                .collect::<Vec<_>>();

            let inferred = facts.get(name).map_or(&[][..], Vec::as_slice);

            let syntax = instar_lint::lint(
                Origin::Module(module.node.module.clone()),
                &Source {
                    text: &module.node.document.text,
                    revision: module.node.document.revision,
                    configuration: &module.settings.configuration.lint,
                    globals: &module.settings.snapshot.globals,
                    roblox: module.settings.configuration.roblox.enabled == Some(true),
                    requires: &requires,
                    inferred: native_requested.then_some(inferred),
                },
                &remaining(options, started),
            )?;

            if syntax.completion != Completion::Complete {
                result.completion = syntax.completion;
            }

            let native_syntax = result.diagnostics.iter().any(|existing| {
                existing.kind == Kind::Syntax
                    && existing.location.module == Origin::Module(module.node.module.clone())
            });

            for diagnostic in syntax.diagnostics {
                if diagnostic.kind == Kind::Syntax && native_syntax {
                    continue;
                }

                if !result.diagnostics.contains(&diagnostic) {
                    result.diagnostics.push(diagnostic);
                }
            }
        }

        Ok(())
    }

    fn native(
        &self,
        native: LintResult,
        facts: &mut BTreeMap<String, Vec<Inference>>,
        result: &mut instar_lint::Result<Origin>,
    ) -> io::Result<()> {
        if native.completion != Completion::Complete {
            result.completion = native.completion;
        }

        for fact in native.facts {
            let location = locate(&fact.location, &self.sources)?;

            facts
                .entry(fact.location.module)
                .or_default()
                .push(Inference {
                    range: location.range,
                    parameter: fact.kind == FactKind::ImplicitAnyParameter,
                });
        }

        for warning in native.warnings {
            let location = locate(&warning.location, &self.sources)?;

            if !self
                .modules
                .get(&warning.location.module)
                .is_some_and(|module| module.selected)
            {
                continue;
            }

            let diagnostic = Diagnostic {
                location,
                kind: Kind::Native {
                    code: warning.code,
                    name: warning.name,
                },
                level: if warning.fatal {
                    Level::Deny
                } else {
                    Level::Warn
                },
                message: warning.message,
                related: Vec::new(),
            };

            if !result.diagnostics.contains(&diagnostic) {
                result.diagnostics.push(diagnostic);
            }
        }

        for diagnostic in native.diagnostics {
            let location = locate(&diagnostic.location, &self.sources)?;

            let kind = match diagnostic.kind {
                instar_check::Kind::Syntax { .. } => Kind::Syntax,
                instar_check::Kind::Resolution { .. } => Kind::Resolution,
                instar_check::Kind::Unsupported => Kind::Unsupported,
                instar_check::Kind::Analysis { .. } => Kind::Analysis,

                instar_check::Kind::Type { .. }
                    if matches!(location.module, Origin::Definition(_)) =>
                {
                    Kind::Analysis
                }

                instar_check::Kind::Type { .. } => {
                    return Err(invalid(
                        "native lint returned an ordinary checker type diagnostic",
                    ));
                }
            };

            if kind == Kind::Resolution
                && result.diagnostics.iter().any(|existing| {
                    existing.kind == Kind::Resolution
                        && existing.location.module == location.module
                        && existing.location.range[0] <= location.range[1]
                        && location.range[0] <= existing.location.range[1]
                })
            {
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
                kind,
                level: Level::Deny,
                message: diagnostic.message,
                related,
            };

            if !result.diagnostics.contains(&diagnostic) {
                result.diagnostics.push(diagnostic);
            }
        }

        Ok(())
    }
}
