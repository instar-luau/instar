use std::{
    collections::{BTreeMap, btree_map::Entry},
    iter,
    time::Instant,
};

use eyre::{Result, eyre};
use instar_bridge::{Context, Frontend, Progress};
use instar_core::workspace::{Purpose, Workspace};

use super::{
    arguments::Operation,
    batch::{Batch, Finding, Label, Severity, Source},
    execution::{self, Outcome, Project, Response},
    interrupt,
    watch::Watcher,
};

pub(super) fn session(
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    tracing::debug!("Creating native frontend");
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

            tracing::debug!(checks = ?frontend.checks().checked_sub(checks), "Completed native checks");

            Ok(Response::Completed(batch))
        },
    )
}

fn process(
    workspace: &Workspace,
    module: &str,
    frontend: &mut Frontend<'_>,
    files: &mut BTreeMap<String, (Source, Vec<usize>)>,
) -> Result<Batch> {
    let mut batch = Batch::default();

    for diagnostic in frontend.check(module)? {
        if !diagnostic.module.is_empty()
            && workspace.is_excluded(&diagnostic.module, Purpose::Check)?
        {
            continue;
        }

        let mut finding = Finding::new(
            Severity::Error,
            diagnostic.code.to_string(),
            diagnostic.message,
            Vec::new(),
        );

        if !diagnostic.module.is_empty() {
            let file = match files.entry(diagnostic.module.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),

                Entry::Vacant(entry) => {
                    let source = workspace.snapshot(&diagnostic.module).ok_or_else(|| {
                        eyre!("Could not read diagnostic source `{}`.", diagnostic.module)
                    })?;

                    let lines = iter::once(0)
                        .chain(source.match_indices('\n').map(|(offset, _)| offset + 1))
                        .collect();

                    entry.insert((
                        Source {
                            module: diagnostic.module.clone(),
                            text: source,
                        },
                        lines,
                    ))
                }
            };

            let start = file.1.get(diagnostic.start_line as usize).ok_or_else(|| {
                eyre!("Internal error: could not locate the start of the diagnostic.")
            })? + diagnostic.start_column as usize;

            let end = file.1.get(diagnostic.end_line as usize).ok_or_else(|| {
                eyre!("Internal error: could not locate the end of the diagnostic.")
            })? + diagnostic.end_column as usize;

            finding.labels.push(Label::new(file.0.clone(), start..end));
        }

        batch.findings.push(finding);
    }

    Ok(batch)
}
