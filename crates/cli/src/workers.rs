use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use eyre::{Report, Result, eyre};
use indicatif::ProgressBar;
use instar_core::workspace::Workspace;

use super::{
    arguments::{Arguments, Command},
    execution::{self, Project, Response},
    report::Batch,
};

struct Request {
    dirty: BTreeSet<String>,
    progress: ProgressBar,
}

pub(super) struct Worker<'modules> {
    modules: &'modules [String],
    requests: Receiver<Request>,
    responses: Sender<Response>,
}

impl Worker<'_> {
    pub(super) fn run(
        self,
        workspace: &Workspace,
        mut execute: impl FnMut(&[String], &ProgressBar) -> Result<Batch>,
    ) -> Result<()> {
        for request in self.requests {
            for module in &request.dirty {
                if workspace.requires_reload(module)? {
                    self.responses.send(Response::Reload)?;

                    return Ok(());
                }

                workspace.invalidate(module);
            }

            let mut batch = execute(self.modules, &request.progress)?;
            batch.dependencies = workspace.dependencies();

            if self.responses.send(Response::Completed(batch)).is_err() {
                break;
            }
        }

        Ok(())
    }
}

struct Connection {
    requests: Sender<Request>,
    responses: Receiver<Response>,
}

pub(super) struct Pool {
    connections: Vec<Connection>,
}

impl Pool {
    pub(super) fn execute(
        &self,
        dirty: &BTreeSet<String>,
        progress: &ProgressBar,
    ) -> Result<Response> {
        for connection in &self.connections {
            connection.requests.send(Request {
                dirty: dirty.clone(),
                progress: progress.clone(),
            })?;
        }

        let mut batch = Batch::default();
        let mut reload = false;

        for connection in &self.connections {
            match connection.responses.recv()? {
                Response::Completed(completed) => batch.append(completed),
                Response::Reload => reload = true,
            }
        }

        Ok(if reload {
            Response::Reload
        } else {
            Response::Completed(batch)
        })
    }
}

pub(super) fn run(
    arguments: &Arguments,
    project: &Project,
    execute: impl Fn(&Workspace, Worker<'_>) -> Result<()> + Sync,
    session: impl FnOnce(&Pool) -> Result<u8>,
) -> Result<u8> {
    let requested = match &arguments.command {
        Command::Check(_) => None,
        Command::Format(format) => format.jobs,
        Command::Lint(lint) => lint.jobs,
    };

    let modules: Vec<_> = project.selected.keys().cloned().collect();

    let jobs = requested
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
        .min(modules.len())
        .max(1);

    let root = project.workspace.directory();
    let options = arguments.command.options();
    let overrides = &project.overrides;
    let stdin = project.stdin.as_ref();

    if arguments.verbose {
        eprintln!("worker limit: {jobs}");
    }

    let operation = |worker: Worker<'_>| -> Result<()> {
        let span = tracing::debug_span!("worker", files = worker.modules.len());
        let _entered = span.enter();
        let (workspace, _) = execution::open(root, options, overrides, stdin)?;

        execute(&workspace, worker)
    };

    thread::scope(|scope| {
        let mut threads = Vec::new();
        let mut failure = None;

        let result = {
            let mut pool = Pool {
                connections: Vec::new(),
            };

            for modules in modules
                .chunks(modules.len().div_ceil(jobs).max(1))
                .chain(modules.is_empty().then_some(modules.as_slice()))
            {
                let (requests, incoming) = mpsc::channel();
                let (outgoing, responses) = mpsc::channel();

                let worker = Worker {
                    modules,
                    requests: incoming,
                    responses: outgoing,
                };

                let operation = &operation;

                match thread::Builder::new().spawn_scoped(scope, move || operation(worker)) {
                    Ok(thread) => {
                        threads.push(thread);

                        pool.connections.push(Connection {
                            requests,
                            responses,
                        });
                    }

                    Err(error) => {
                        failure = Some(Report::from(error));
                        break;
                    }
                }
            }

            if failure.is_some() {
                Ok(0)
            } else {
                session(&pool)
            }
        };

        for thread in threads {
            match thread.join() {
                Ok(Ok(())) => {}

                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }

                Err(panic) => {
                    let reason = panic
                        .downcast_ref::<String>()
                        .map(String::as_str)
                        .or_else(|| panic.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic payload");

                    failure.get_or_insert_with(|| eyre!("worker panicked: {reason}"));
                }
            }
        }

        failure.map_or(result, Err)
    })
}
