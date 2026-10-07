//! Lint orchestration over shared sources, contextual resolution and native analysis.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    rc::Rc,
    time::Instant,
};

use instar_analysis::error::invalid;
use instar_bridge::frontend::{Definition, FactKind, LintResult};
use instar_lint::{Completion, Diagnostic, Inference, Kind, Level, Options, Require, Source};

use crate::{
    analysis::{Entry, Origin, Report, locate},
    native,
    project::{Project, Settings, absolute},
    resolve::Request,
};

struct Module {
    node: Rc<crate::graph::Node>,
    settings: Rc<Settings>,
}

#[derive(Default)]
struct Analysis {
    roots: BTreeMap<String, crate::resolve::Module>,
    report: Report,
    modules: BTreeMap<String, Module>,
    definitions: BTreeMap<String, Vec<Definition>>,
}

impl Project {
    /// Lints selected contextual sources using independent native and Instar policies.
    ///
    /// Include/exclude settings select root entries. All reachable modules contribute
    /// findings according to their own rule settings. Native warnings use the shared
    /// native frontend and declaration environments. Instar syntax rules do not load
    /// declarations when no native warning or inferred-type rule is requested.
    /// Roblox environments load the configured generated asset bundle.
    /// Native analysis is isolated in a cancellable worker process.
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

        if let Some(reason) = options.interrupted(started) {
            result.completion = Completion::Incomplete(reason);

            result
                .diagnostics
                .extend(analysis.report.diagnostics.into_iter().map(Into::into));

            return Ok(result);
        }

        let semantic = analysis
            .modules
            .iter()
            .filter(|(_, module)| module.settings.configuration.lint.semantic())
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();

        let native_requested = !semantic.is_empty()
            || analysis.modules.values().any(|module| {
                module
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

        analysis.syntax(&facts, &mut result, options, started)?;

        if let Some(reason) = options.interrupted(started) {
            result.completion = Completion::Incomplete(reason);
        }

        result
            .diagnostics
            .extend(analysis.report.diagnostics.into_iter().map(Into::into));

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
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
                break;
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
            let node = self
                .graph
                .node(&identity)
                .ok_or_else(|| invalid("lint module is missing from the shared graph"))?;

            result.modules.push(analysis.report.collect(&node));

            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
                continue;
            }

            self.lint_module(node, analysis)?;
        }

        Ok(())
    }

    fn lint_module(
        &mut self,
        node: Rc<crate::graph::Node>,
        analysis: &mut Analysis,
    ) -> io::Result<()> {
        let directory = node
            .module
            .source
            .parent()
            .ok_or_else(|| invalid("lint module has no directory"))?;

        let settings = self.configuration(directory)?;

        analysis.modules.insert(
            native::name(&node.module.identity),
            Module { node, settings },
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
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);

                return Ok(facts);
            }

            self.install(&module.source, Some(&module.identity))?;
        }

        self.lint_definitions(analysis, result, options, started);

        if let Some(reason) = options.interrupted(started) {
            result.completion = Completion::Incomplete(reason);

            return Ok(facts);
        }

        if result.completion != Completion::Complete {
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
        let limits = options.remaining(started);

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
            let environment = self.environment(
                &module.node,
                &module.settings,
                &mut analysis.report.sources,
                options,
                started,
            );

            result.completion = result.completion.combine(environment.completion);

            for diagnostic in environment.diagnostics {
                analysis.report.record(diagnostic);
            }

            analysis
                .definitions
                .insert(name.clone(), environment.definitions);

            if options.interrupted(started).is_some() {
                return;
            }
        }
    }
}

impl Analysis {
    fn syntax(
        &self,
        facts: &BTreeMap<String, Vec<Inference>>,
        result: &mut instar_lint::Result<Origin>,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        for (name, module) in &self.modules {
            if let Some(reason) = options.interrupted(started) {
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

            let inferred = facts.get(name).map(Vec::as_slice);

            let syntax = instar_lint::lint(
                Origin::Module(module.node.module.clone()),
                &Source {
                    text: &module.node.document.text,
                    revision: module.node.document.revision,
                    configuration: &module.settings.configuration.lint,
                    globals: &module.settings.snapshot.globals,
                    roblox: module.settings.configuration.roblox.enabled == Some(true),
                    requires: &requires,
                    inferred,
                },
                &options.remaining(started),
            )?;

            result.completion = result.completion.combine(syntax.completion);

            let native_syntax = self.report.diagnostics.iter().any(|existing| {
                matches!(existing.kind, instar_analysis::Kind::Syntax { .. })
                    && existing.location.module == Origin::Module(module.node.module.clone())
            });

            for diagnostic in syntax.diagnostics {
                if matches!(
                    diagnostic.kind,
                    Kind::Analysis(instar_analysis::Kind::Syntax { .. })
                ) && native_syntax
                {
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
        &mut self,
        native: LintResult,
        facts: &mut BTreeMap<String, Vec<Inference>>,
        result: &mut instar_lint::Result<Origin>,
    ) -> io::Result<()> {
        result.completion = result.completion.combine(native.completion);

        if native.completion == Completion::Complete {
            for (name, module) in &self.modules {
                if module.settings.configuration.lint.semantic() {
                    facts.entry(name.clone()).or_default();
                }
            }
        }

        for fact in native.facts {
            let location = locate(&fact.location, &self.report.sources)?;

            facts
                .entry(fact.location.module)
                .or_default()
                .push(Inference {
                    range: location.range,
                    parameter: fact.kind == FactKind::ImplicitAnyParameter,
                });
        }

        for warning in native.warnings {
            let location = locate(&warning.location, &self.report.sources)?;

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
            if matches!(diagnostic.kind, instar_analysis::Kind::Type { .. })
                && !matches!(
                    self.report.sources.get(&diagnostic.location.module),
                    Some((Origin::Definition(_), _))
                )
            {
                return Err(invalid(
                    "native lint returned an ordinary checker type diagnostic",
                ));
            }

            self.report.native(diagnostic)?;
        }

        Ok(())
    }
}

fn eligible(source: &Path, settings: &Settings) -> io::Result<bool> {
    let lint = &settings.configuration.lint;

    Ok(
        instar_analysis::selection::matches(source, lint.include.as_deref(), true)?
            && !instar_analysis::selection::matches(source, lint.exclude.as_deref(), false)?,
    )
}
