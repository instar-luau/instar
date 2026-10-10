use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use instar_analysis::{self as analysis, Completion, Options, error::invalid};

use super::{Entry, Origin, Report};
use crate::{environment::Environment, native, project::Project, resolve::Identity};

#[derive(Default)]
pub(crate) struct Preparation {
    pub(crate) report: Report,
    pub(crate) environments: BTreeMap<String, Rc<Environment>>,
    prepared: BTreeMap<PathBuf, Rc<Environment>>,
}

impl Project {
    pub(crate) fn semantic(
        &mut self,
        entry: &Entry,
        options: &Options,
        started: Instant,
    ) -> io::Result<(String, Preparation)> {
        let mut identities = BTreeSet::new();
        let module = self.install(&entry.source, entry.context.as_ref(), &mut identities)?;
        let mut preparation = Preparation::default();

        let mut result = analysis::Result {
            modules: Vec::new(),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        };

        self.prepare_modules(identities, &mut preparation, &mut result, options, started)?;

        if let Completion::Incomplete(reason) = result.completion {
            return Err(reason.error());
        }

        let frontend = self
            .frontend
            .as_mut()
            .ok_or_else(|| invalid("shared native frontend is unavailable"))?;

        if let Completion::Incomplete(reason) =
            native::definitions(&preparation.environments, frontend, options, started)?
        {
            return Err(reason.error());
        }

        Ok((native::name(&module.identity), preparation))
    }

    pub(crate) fn prepare_modules(
        &mut self,
        identities: BTreeSet<Identity>,
        preparation: &mut Preparation,
        result: &mut analysis::Result<Origin>,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        for identity in &identities {
            let node = self
                .graph
                .node(identity)
                .ok_or_else(|| invalid("analysis module is missing from the Rust graph"))?;

            result.modules.push(preparation.report.collect(&node));
        }

        let total = identities.len();

        for (completed, identity) in identities.into_iter().enumerate() {
            if let Some(reason) = options.interrupted(started) {
                result.completion = Completion::Incomplete(reason);
                break;
            }

            let source = result.modules[completed].source().to_string_lossy();

            options.report(
                "Preparing environments",
                completed,
                Some(total),
                Some(&source),
            );

            self.prepare_module(&identity, preparation, result, options, started)?;

            if result.completion == Completion::Complete {
                options.report(
                    "Preparing environments",
                    completed + 1,
                    Some(total),
                    Some(&result.modules[completed].source().to_string_lossy()),
                );
            }
        }

        Ok(())
    }

    fn prepare_module(
        &mut self,
        identity: &Identity,
        preparation: &mut Preparation,
        result: &mut analysis::Result<Origin>,
        options: &Options,
        started: Instant,
    ) -> io::Result<()> {
        let node = self
            .graph
            .node(identity)
            .ok_or_else(|| invalid("analysis module is missing from the Rust graph"))?;

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
            &mut preparation.report.sources,
            &mut preparation.prepared,
            options,
            started,
        );

        result.completion = result.completion.combine(environment.completion);

        for diagnostic in environment.diagnostics.iter().cloned() {
            preparation.report.record(diagnostic);
        }

        preparation.environments.insert(name, environment);

        Ok(())
    }
}
