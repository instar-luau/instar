use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex, Weak,
        mpsc::{Receiver, Sender},
    },
};

use instar_bridge::{Context, Frontend, Progress};
use tower_lsp_server::jsonrpc::{Error, Result};

use super::{
    connection::{Completion, Message, Operation, Work},
    files,
    project::{Project, failure},
    workspace::History,
};

struct Cached {
    project: Project,
    sequence: u64,
}

pub(super) fn run(
    receiver: &Receiver<Work>,
    sender: &Sender<Message>,
    current: &Mutex<Weak<Operation>>,
) {
    let mut projects = BTreeMap::<PathBuf, Cached>::new();
    let mut pending: Option<Work> = None;

    loop {
        let Ok(work) = pending.take().map_or_else(|| receiver.recv(), Ok) else {
            return;
        };

        let root = work.snapshot.root.clone();

        let cached = projects
            .remove(&root)
            .filter(|cached| reusable(&cached.project, cached.sequence, &work));

        let (project, mut sequence) = cached.map_or_else(
            || {
                (
                    Project::new(
                        &root,
                        work.snapshot.configuration.clone(),
                        Arc::clone(&work.snapshot.capabilities),
                        work.snapshot.encoding,
                    ),
                    work.history.sequence,
                )
            },
            |cached| (Ok(cached.project), cached.sequence),
        );

        let mut project = match project {
            Ok(project) => project,

            Err(error) => {
                let (success, deliver) = (work.action)(Err(error));

                drop(sender.send(Message::Complete(Completion {
                    identifier: work.identifier,
                    feedback: None,
                    success,
                    deliver,
                })));

                continue;
            }
        };

        let workspace = Rc::clone(&project.workspace);
        let operation = RefCell::new(Arc::clone(&work.operation));
        let update = |stage: Progress<'_>| operation.borrow().report(stage);

        let discarded = {
            let cancelled = Arc::clone(&work.operation);
            workspace.set_cancellation(move || cancelled.requested());

            *current
                .lock()
                .expect("internal error: active operation poisoned") =
                Arc::downgrade(&work.operation);

            let context = Context::new(workspace.as_ref()).with_progress(&update);
            let mut frontend = Frontend::new(&context, true);

            let cancellation = frontend
                .as_ref()
                .map_err(failure)
                .and_then(|frontend| frontend.cancellation().map_err(failure));

            let mut next = work;

            loop {
                *current
                    .lock()
                    .expect("internal error: active operation poisoned") =
                    Arc::downgrade(&next.operation);

                *operation.borrow_mut() = Arc::clone(&next.operation);
                let cancelled = Arc::clone(&next.operation);
                workspace.set_cancellation(move || cancelled.requested());

                if let Ok(cancellation) = &cancellation {
                    next.operation.attach(cancellation);
                }

                let mut discarded = false;

                let session = if next.operation.requested() {
                    Err(Error::request_cancelled())
                } else {
                    match &cancellation {
                        Ok(_) => frontend.as_mut().map_err(failure).and_then(|frontend| {
                            if let Err(error) =
                                invalidate(&mut project, sequence, &next.history, frontend)
                            {
                                discarded = true;

                                return Err(error);
                            }

                            let applied = next.history.sequence;

                            if let Err(error) = project.apply(next.snapshot, frontend) {
                                discarded = true;

                                return Err(error);
                            }

                            sequence = applied;

                            Ok((&mut project, frontend))
                        }),

                        Err(error) => Err(error.clone()),
                    }
                };

                let (success, deliver) = (next.action)(session);
                next.operation.detach();
                workspace.set_cancellation(|| false);

                *current
                    .lock()
                    .expect("internal error: active operation poisoned") = Weak::new();

                if sender
                    .send(Message::Complete(Completion {
                        identifier: next.identifier,
                        feedback: Some(project.feedback()),
                        success,
                        deliver,
                    }))
                    .is_err()
                {
                    return;
                }

                if discarded || cancellation.is_err() {
                    break discarded;
                }

                match receiver.recv() {
                    Ok(work)
                        if work.snapshot.root == root && reusable(&project, sequence, &work) =>
                    {
                        next = work;
                    }

                    Ok(work) => {
                        pending = Some(work);
                        break false;
                    }

                    Err(_) => return,
                }
            }
        };

        if !discarded {
            projects.insert(root, Cached { project, sequence });
        }
    }
}

fn reusable(project: &Project, sequence: u64, work: &Work) -> bool {
    if project.invalidation != work.snapshot.invalidation
        || project.indexed != work.snapshot.indexed
        || project.nested != work.snapshot.nested
    {
        return false;
    }

    if sequence == work.history.sequence {
        return true;
    }

    let Some(mut changes) = work.history.since(sequence) else {
        return false;
    };

    let Ok(dependencies) = project
        .reload_dependencies()
        .iter()
        .map(|path| files::identity(path))
        .collect::<Result<Vec<_>>>()
    else {
        return false;
    };

    changes.all(|change| {
        change.paths.iter().all(|path| {
            !(path.file_name().is_some_and(|name| name == "instar.toml")
                && path.starts_with(project.workspace.directory())
                || dependencies
                    .iter()
                    .any(|dependency| intersects(dependency, path)))
        })
    })
}

fn invalidate(
    project: &mut Project,
    sequence: u64,
    history: &History,
    frontend: &mut Frontend<'_>,
) -> Result<()> {
    if sequence == history.sequence {
        return Ok(());
    }

    let changes = history
        .since(sequence)
        .ok_or_else(Error::content_modified)?;

    for change in changes {
        let modules: BTreeSet<_> = project
            .dependencies()
            .into_iter()
            .chain(project.documents.keys().map(PathBuf::from))
            .chain(project.borrowed.keys().map(PathBuf::from))
            .filter(|module| change.paths.iter().any(|path| intersects(module, path)))
            .filter(|module| {
                module
                    .extension()
                    .is_some_and(|extension| extension == "luau" || extension == "lua")
            })
            .collect();

        for module in modules {
            let module = project.workspace.normalize(&module).map_err(failure)?;
            project.invalidate(&module);
            frontend.invalidate(&module).map_err(failure)?;
        }
    }

    frontend.refresh().map_err(failure)?;

    Ok(())
}

fn intersects(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}
