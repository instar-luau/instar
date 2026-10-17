use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::Arc,
    time::Instant,
};

use codespan_reporting::{
    diagnostic::{Diagnostic, Label},
    files::{Files as _, SimpleFile},
};

use eyre::{Result, WrapErr as _, eyre};
use instar_core::workspace::{Purpose, Workspace};
use instar_luau::{Context, Frontend, Progress};

use super::{
    arguments::{Arguments, Operation},
    execution::{self, Outcome, Project, Response},
    interrupt,
    report::{Batch, Source},
    watch::Watcher,
};

pub(super) fn run(
    arguments: &Arguments,
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    tracing::debug!("creating native frontend");
    let workspace = &project.workspace;

    let update = |stage: Progress<'_>| {
        match stage {
            Progress::Started(module) => project.progress.start(module),
            Progress::Finished(module) => project.progress.finish(module),
            Progress::Advanced { completed, total } => project.progress.advance(completed, total),
        }

        !interrupt::requested()
    };

    let context = Context::new(workspace).with_progress(&update);
    let mut frontend = Frontend::new(&context, false)?;

    execution::session(
        arguments,
        operation,
        project,
        watcher,
        started,
        |selected, dirty, discovery, _| {
            let checks = frontend.checks();

            for module in dirty {
                frontend.invalidate(module)?;
            }

            if discovery {
                frontend.refresh()?;
            }

            let mut files = BTreeMap::new();
            let modules: Vec<_> = selected.keys().cloned().collect();

            let mut queued = Vec::with_capacity(modules.len());

            let mut batch = Batch::collect(&modules, |module| {
                frontend.queue(module)?;
                queued.push(module.to_owned());

                Ok(Batch::default())
            })?;

            batch.append(Batch::collect(&queued, |module| {
                process(workspace, module, &mut frontend, &mut files)
            })?);

            tracing::debug!(checks = ?frontend.checks().checked_sub(checks), "completed native checks");

            Ok(Response::Completed(batch))
        },
    )
}

fn process(
    workspace: &Workspace,
    module: &str,
    frontend: &mut Frontend<'_>,
    files: &mut BTreeMap<String, SimpleFile<String, Arc<str>>>,
) -> Result<Batch> {
    let mut batch = Batch::default();

    for diagnostic in frontend.check(module)? {
        if !diagnostic.module.is_empty()
            && workspace.excluded(&diagnostic.module, Purpose::Check)?
        {
            continue;
        }

        let mut finding = Diagnostic::error()
            .with_code(diagnostic.code.to_string())
            .with_message(diagnostic.message);

        if !diagnostic.module.is_empty() {
            let file = match files.entry(diagnostic.module.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),

                Entry::Vacant(entry) => {
                    let source = workspace.snapshot(&diagnostic.module).ok_or_else(|| {
                        eyre!("cannot read diagnostic source `{}`", diagnostic.module)
                    })?;

                    entry.insert(SimpleFile::new(diagnostic.module.clone(), source))
                }
            };

            let start = file
                .line_range((), diagnostic.start_line as usize)
                .wrap_err("internal error: cannot locate diagnostic start")?
                .start
                + diagnostic.start_column as usize;

            let end = file
                .line_range((), diagnostic.end_line as usize)
                .wrap_err("internal error: cannot locate diagnostic end")?
                .start
                + diagnostic.end_column as usize;

            let source = Source {
                module: diagnostic.module,
                text: Arc::clone(file.source()),
            };

            finding.labels.push(Label::primary(source, start..end));
        }

        batch.findings.push(finding);
    }

    Ok(batch)
}
