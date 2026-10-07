//! Native checking over the shared project source and dependency graph.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use instar_analysis::{Completion, Options, error::invalid};

use crate::{
    analysis::{Entry, Origin, Report},
    environment::Environment,
    native,
    project::{Project, absolute},
    resolve::Identity,
};

#[derive(Default)]
struct Analysis {
    report: Report,
    environments: BTreeMap<String, Rc<Environment>>,
    prepared: BTreeMap<PathBuf, Rc<Environment>>,
}

impl Project {
    /// Checks selected entries and reports dependency errors once per exact module.
    ///
    /// Instar patterns select entries only. Native configuration controls language
    /// modes and type error visibility; native lint warnings are never emitted.
    /// Roblox environments use automatically downloaded and verified assets.
    /// Native analysis is isolated in a cancellable worker process.
    /// Missing declaration files use revision zero and an empty source range.
    ///
    /// # Errors
    /// Returns source/configuration failures and invalid host/native source agreement.
    pub fn check(
        &mut self,
        entries: &[Entry],
        options: &Options,
    ) -> io::Result<instar_analysis::Result<Origin>> {
        let started = Instant::now();

        let mut result = instar_analysis::Result {
            modules: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        let mut roots = BTreeSet::new();
        let mut identities = BTreeSet::new();

        let preparation = self.during(&options.remaining(started), |project| {
            for entry in entries {
                if let Some(reason) = options.interrupted(started) {
                    result.completion = Completion::Incomplete(reason);
                    break;
                }

                let source = absolute(&entry.source)?;

                let directory = source
                    .parent()
                    .ok_or_else(|| invalid("checker entry has no directory"))?;

                let settings = project.configuration(directory)?;

                if !instar_analysis::selection::matches(
                    &source,
                    settings.configuration.check.include.as_deref(),
                    true,
                )? || instar_analysis::selection::matches(
                    &source,
                    settings.configuration.check.exclude.as_deref(),
                    false,
                )? {
                    continue;
                }

                let module = project.install(&source, entry.context.as_ref())?;
                roots.insert(native::name(&module.identity));
                identities.extend(project.discover(&source, entry.context.as_ref())?);
            }

            Ok(())
        });

        if let Err(error) = preparation {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
            } else {
                return Err(error);
            }
        }

        if roots.is_empty() {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
            }

            return Ok(result);
        }

        let mut analysis = Analysis::default();

        for identity in &identities {
            let node = self
                .graph
                .node(identity)
                .ok_or_else(|| invalid("checker module is missing from the Rust graph"))?;

            result.modules.push(analysis.report.collect(&node));
        }

        for identity in identities {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
                break;
            }

            self.check_module(&identity, options, started, &mut analysis, &mut result)?;
        }

        if result.completion != Completion::Complete {
            result.diagnostics = analysis.report.diagnostics;

            return Ok(result);
        }

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        for (name, environment) in &analysis.environments {
            frontend.definitions(name, &environment.definitions)?;
        }

        let remaining = options.remaining(started);
        let checked = frontend.check(&roots.into_iter().collect::<Vec<_>>(), &remaining)?;
        result.completion = result.completion.combine(checked.completion);

        for diagnostic in checked.diagnostics {
            analysis.report.native(diagnostic)?;
        }

        result.diagnostics = analysis.report.diagnostics;

        if let Some(reason) = options.interrupted(started) {
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
        result: &mut instar_analysis::Result<Origin>,
    ) -> io::Result<()> {
        let node = self
            .graph
            .node(identity)
            .ok_or_else(|| invalid("checker module is missing from the Rust graph"))?;

        let name = native::name(identity);

        let directory = node
            .module
            .source
            .parent()
            .ok_or_else(|| invalid("module source has no directory"))?;

        let settings = self.configuration(directory)?;

        let environment = self.environment(
            &node,
            &settings,
            &mut analysis.report.sources,
            &mut analysis.prepared,
            options,
            started,
        );

        result.completion = result.completion.combine(environment.completion);

        for diagnostic in environment.diagnostics.iter().cloned() {
            analysis.report.record(diagnostic);
        }

        analysis.environments.insert(name, environment);

        Ok(())
    }
}
