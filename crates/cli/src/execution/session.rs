use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    path::PathBuf,
    time::Instant,
};

use eyre::Result;

use super::{
    project::{Project, select},
    watch::{Change, Watcher},
};

use crate::{
    arguments::Operation,
    batch::Batch,
    interrupt,
    output::{self, Progress},
};

pub(crate) enum Response {
    Completed(Batch),
    Reload,
}

pub(crate) enum Outcome {
    Completed(u8),

    Reload { started: Instant },
}

pub(crate) fn session(
    operation: &Operation,
    project: &Project,
    mut watcher: Option<&mut Watcher>,
    mut started: Instant,
    mut execute: impl FnMut(
        &BTreeMap<String, PathBuf>,
        &BTreeSet<String>,
        bool,
        &Progress,
    ) -> Result<Response>,
) -> Result<Outcome> {
    let mut dirty = BTreeSet::new();
    let mut selected = project.selected.clone();
    let mut should_discover = false;

    if let Some(watcher) = &mut watcher {
        watcher.begin(&project.workspace)?;
    }

    loop {
        tracing::debug!("Processing batch");

        let progress = &project.progress;
        progress.reset(selected.len());
        let result = execute(&selected, &dirty, should_discover, progress);
        progress.clear();
        interrupt::check()?;

        let mut batch = match result? {
            Response::Completed(batch) => batch,

            Response::Reload => {
                return Ok(Outcome::Reload { started });
            }
        };

        if let Some(watcher) = &mut watcher {
            batch.dependencies.extend(project.workspace.dependencies());

            batch
                .reload_dependencies
                .extend(project.workspace.reload_dependencies());

            watcher.observe(mem::take(&mut batch.reload_dependencies), true)?;
            watcher.observe(mem::take(&mut batch.dependencies), false)?;
        }

        let result = output::publish(batch, operation, project);
        progress.clear();
        let summary = result?;

        output::summary(&summary, operation, progress.files(), started);
        let status = summary.status(operation);

        let Some(watcher) = &mut watcher else {
            return Ok(Outcome::Completed(status));
        };

        output::watching();

        loop {
            let (paths, has_structural_changes, changed) =
                match watcher.changed(&project.workspace)? {
                    Change::Sources {
                        paths,
                        should_discover,
                        started,
                    } => (paths, should_discover, started),

                    Change::Reload { started } => {
                        return Ok(Outcome::Reload { started });
                    }
                };

            started = changed;

            let previous = if has_structural_changes {
                mem::replace(
                    &mut selected,
                    select(
                        operation,
                        &project.workspace,
                        project.stdin_module.as_deref(),
                    )?,
                )
            } else {
                BTreeMap::new()
            };

            let mut candidates = project.workspace.dependencies();
            candidates.extend(selected.keys().chain(previous.keys()).map(PathBuf::from));
            dirty.clear();
            should_discover = false;

            for candidate in candidates {
                let module = project.workspace.normalize(&candidate)?;

                let has_membership_changed = has_structural_changes
                    && selected.contains_key(&module) != previous.contains_key(&module);

                if !has_membership_changed && !paths.iter().any(|path| candidate.starts_with(path))
                {
                    continue;
                }

                should_discover |= has_structural_changes;

                if candidate
                    .extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
                    && !candidate.is_dir()
                    && (project.workspace.refresh(&module)? || has_membership_changed)
                {
                    dirty.insert(module);
                }
            }

            if !dirty.is_empty()
                || should_discover
                || (has_structural_changes && selected != previous)
            {
                if has_structural_changes {
                    watcher.begin(&project.workspace)?;
                }

                break;
            }

            tracing::debug!("Ignoring filesystem changes outside active inputs");
        }
    }
}
