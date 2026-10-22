use std::{
    collections::BTreeSet,
    num::NonZeroUsize,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Instant,
};

use eyre::{Report, Result, WrapErr as _, eyre};
use instar_core::workspace::Workspace;

use super::{
    project::{self, Project},
    session::{self, Outcome, Response},
    watch::Watcher,
};

use crate::{
    arguments::Operation,
    batch::Batch,
    output::{self, Progress},
};

struct Request {
    dirty: BTreeSet<String>,
    progress: Progress,
}

struct Worker<'modules> {
    modules: &'modules [String],
    requests: Receiver<Request>,
    responses: Sender<Result<Batch>>,
}

impl Worker<'_> {
    fn run(
        self,
        workspace: &Workspace,
        mut execute: impl FnMut(&[String], &Progress) -> Result<Batch>,
    ) {
        for request in self.requests {
            for module in &request.dirty {
                workspace.invalidate(module);
            }

            let result = execute(self.modules, &request.progress).map(|mut batch| {
                batch.dependencies = workspace.dependencies();
                batch.reload_dependencies = workspace.reload_dependencies();

                batch
            });

            if self.responses.send(result).is_err() {
                break;
            }
        }
    }
}

struct Connection {
    requests: Sender<Request>,
    responses: Receiver<Result<Batch>>,
}

struct Pool {
    connections: Vec<Connection>,
}

impl Pool {
    fn execute(&self, dirty: &BTreeSet<String>, progress: &Progress) -> Result<Batch> {
        for connection in &self.connections {
            connection
                .requests
                .send(Request {
                    dirty: dirty.clone(),
                    progress: progress.clone(),
                })
                .wrap_err("Could not send the worker request.")?;
        }

        let mut batch = Batch::default();
        let mut failure = None;

        for connection in &self.connections {
            match connection
                .responses
                .recv()
                .wrap_err("Could not receive the worker response.")?
            {
                Ok(completed) => batch.append(completed),

                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }

        if let Some(error) = failure {
            Err(error)
        } else {
            Ok(batch)
        }
    }
}

pub(crate) fn session(
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
    execute: impl Fn(&Workspace, &[String], &Progress) -> Result<Batch> + Sync,
) -> Result<Outcome> {
    let requested = match operation {
        Operation::Check(_) => None,
        Operation::Format(format) => format.jobs,
        Operation::Lint(lint) => lint.jobs,
    };

    let modules: Vec<_> = project.selected.keys().cloned().collect();

    let jobs = requested
        .or_else(|| thread::available_parallelism().ok())
        .map_or(1, NonZeroUsize::get)
        .min(modules.len())
        .max(1);

    let root = project.workspace.directory();
    let options = operation.options();
    let overrides = &project.overrides;
    let stdin = project.stdin.as_ref();
    let generators = project.generators.as_ref();
    let generated = project.workspace.generated();

    output::workers(jobs);

    let run_worker = |worker: Worker<'_>| -> Result<()> {
        let span = tracing::debug_span!("worker", files = worker.modules.len());
        let _entered = span.enter();
        let (mut workspace, _) = project::open(root, options, overrides, stdin)?;

        if let Some(generators) = generators {
            workspace = workspace.with_generators(generators.clone());
        }

        workspace.set_generated(generated.clone());

        worker.run(&workspace, |modules, progress| {
            execute(&workspace, modules, progress)
        });

        Ok(())
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

                let run_worker = &run_worker;

                match thread::Builder::new().spawn_scoped(scope, move || run_worker(worker)) {
                    Ok(thread) => {
                        threads.push(thread);

                        pool.connections.push(Connection {
                            requests,
                            responses,
                        });
                    }

                    Err(error) => {
                        failure = Some(Report::from(error).wrap_err("Could not start a worker."));
                        break;
                    }
                }
            }

            if failure.is_some() {
                Ok(Outcome::Completed(0))
            } else {
                session::session(
                    operation,
                    project,
                    watcher,
                    started,
                    |selected, dirty, _, progress| {
                        if selected != &project.selected {
                            return Ok(Response::Reload);
                        }

                        pool.execute(dirty, progress).map(Response::Completed)
                    },
                )
            }
        };

        if (result.is_err() || failure.is_some())
            && let Some(generators) = generators
            && let Err(error) = project::shutdown(generators)
        {
            failure.get_or_insert(error);
        }

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
                        .unwrap_or("Unknown panic payload.");

                    failure.get_or_insert_with(|| {
                        eyre!("Internal error: a worker panicked: {reason}")
                    });
                }
            }
        }

        failure.map_or(result, Err)
    })
}
