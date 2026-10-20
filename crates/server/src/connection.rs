use std::{
    collections::{BTreeMap, VecDeque},
    fs, mem,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use instar_bridge::{Cancellation, Frontend, Progress};

use tokio::{
    runtime::Handle,
    sync::{OwnedSemaphorePermit, oneshot},
    task,
};

use tower_lsp_server::{
    Client,
    gen_lsp_types::{ProgressToken, Range as ProtocolRange, TextEdit, Uri},
    jsonrpc::{Error, Result},
};

use super::{
    editing, files, native,
    output::progress,
    project::{Feedback, Project, Snapshot, failure},
    workspace::{Documents, History, State},
};

type Source = Box<dyn FnOnce(Result<&mut Project>) + Send>;
type Control = Box<dyn FnOnce(&mut State) + Send>;
pub(super) type Delivery = Box<dyn FnOnce(Result<()>) + Send>;
type Action = Box<dyn FnOnce(Result<(&mut Project, &mut Frontend<'_>)>) -> (bool, Delivery) + Send>;

pub(super) struct Connection {
    sender: Sender<Message>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    current: Arc<Mutex<Weak<Operation>>>,
}

impl Connection {
    pub(super) fn new() -> eyre::Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let (worker, requests) = mpsc::channel();
        let current = Arc::new(Mutex::new(Weak::new()));
        let active = Arc::clone(&current);
        let completed = sender.clone();

        let native = thread::Builder::new()
            .name("native".to_owned())
            .spawn(move || native::run(&requests, &completed, &active))?;

        let source = match thread::Builder::new()
            .name("source".to_owned())
            .spawn(move || run(&receiver, &worker))
        {
            Ok(source) => source,

            Err(error) => {
                drop(native.join());

                return Err(error.into());
            }
        };

        Ok(Self {
            sender,
            threads: Mutex::new(vec![source, native]),
            current,
        })
    }

    pub(super) async fn source<ValueType: Send + 'static>(
        &self,
        uri: Uri,
        operation: Arc<Operation>,
        execute: impl FnOnce(&mut Project) -> Result<ValueType> + Send + 'static,
    ) -> Result<ValueType> {
        let (sender, receiver) = oneshot::channel();
        let pending = Arc::clone(&operation);
        let validated = self.sender.clone();
        let mut guard = Guard(Some(Arc::clone(&operation)));

        self.sender
            .send(Message::Source {
                uri,
                operation: Some(operation),
                execute: Box::new(move |project| {
                    let project = match project {
                        Ok(project) => project,

                        Err(error) => {
                            drop(sender.send(if pending.requested() {
                                Err(Error::request_cancelled())
                            } else {
                                Err(error)
                            }));

                            return;
                        }
                    };

                    let root = project.workspace.directory().to_owned();
                    let generation = project.generation;
                    let invalidation = project.invalidation;

                    let result = if pending.requested() {
                        Err(Error::request_cancelled())
                    } else {
                        execute(project)
                    };

                    drop(validated.send(Message::Control(Box::new(move |state| {
                        let valid = state.projects.get(&root).is_some_and(|project| {
                            project.generation == generation && project.invalidation == invalidation
                        });

                        drop(sender.send(if !valid {
                            Err(Error::content_modified())
                        } else if pending.requested() {
                            Err(Error::request_cancelled())
                        } else {
                            result
                        }));
                    }))));
                }),
            })
            .map_err(failure)?;

        let result = receiver.await.map_err(failure)?;
        guard.0 = None;

        result
    }

    pub(super) async fn formatting(
        &self,
        uri: Uri,
        operation: Arc<Operation>,
        permit: OwnedSemaphorePermit,
        range: Option<ProtocolRange>,
    ) -> Result<Option<Vec<TextEdit>>> {
        let mut guard = Guard(Some(Arc::clone(&operation)));
        let pending = Arc::clone(&operation);

        let (document, encoding, settings, range, ticket, sequence) = self
            .control(move |state| {
                if pending.requested() {
                    return Err(Error::request_cancelled());
                }

                let root = state.select(&uri)?;
                state.synchronize(&root)?;

                let project = state
                    .projects
                    .get_mut(&root)
                    .expect("Internal error: formatting project is missing.");

                project
                    .workspace
                    .set_cancellation(move || pending.requested());

                let captured: Result<_> = (|| {
                    let module = project.module(&uri)?;
                    let document = project.snapshot(&module)?;
                    let encoding = project.encoding;

                    let range = range
                        .map(|range| document.bytes(range, encoding))
                        .transpose()?;

                    let settings = project.workspace.formatting(&module).map_err(failure)?;

                    Ok((document, encoding, settings, range))
                })();

                project.workspace.set_cancellation(|| false);
                let (document, encoding, settings, range) = captured?;
                let ticket = Ticket::capture(state, root);

                Ok((
                    document,
                    encoding,
                    settings,
                    range,
                    ticket,
                    state.history.sequence,
                ))
            })
            .await?;

        let pending = Arc::clone(&operation);

        let result = task::spawn_blocking(move || {
            let result = if pending.requested() {
                Err(Error::request_cancelled())
            } else {
                editing::format(&document, encoding, &settings, range)
            };

            drop(permit);

            result
        })
        .await
        .map_err(failure)?;

        let result = self
            .control(move |state| {
                if !ticket.valid(state) || state.history.sequence != sequence {
                    Err(Error::content_modified())
                } else if operation.requested() {
                    Err(Error::request_cancelled())
                } else {
                    result
                }
            })
            .await;

        guard.0 = None;

        result
    }

    pub(super) async fn query<ValueType: Send + 'static>(
        &self,
        uri: Uri,
        operation: Arc<Operation>,
        execute: impl FnOnce(&mut Project, &mut Frontend<'_>) -> Result<ValueType> + Send + 'static,
    ) -> Result<ValueType> {
        let (sender, receiver) = oneshot::channel();
        let pending = Arc::clone(&operation);
        let mut guard = Guard(Some(Arc::clone(&operation)));

        let action: Action = Box::new(move |session| {
            let result = if pending.requested() {
                Err(Error::request_cancelled())
            } else {
                session.and_then(|(project, frontend)| execute(project, frontend))
            };

            let success = result.is_ok();

            let deliver: Delivery = Box::new(move |accepted| {
                let result = accepted.and_then(|()| {
                    if pending.requested() {
                        Err(Error::request_cancelled())
                    } else {
                        result
                    }
                });

                drop(sender.send(result));
            });

            (success, deliver)
        });

        self.sender
            .send(Message::Query {
                uri,
                operation,
                action,
            })
            .map_err(failure)?;

        let result = receiver.await.map_err(failure)?;
        guard.0 = None;

        result
    }

    pub(super) async fn update<ValueType: Send + 'static>(
        &self,
        uri: Uri,
        execute: impl FnOnce(&mut Project) -> Result<ValueType> + Send + 'static,
    ) -> Result<ValueType> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Message::Source {
                uri,
                operation: None,
                execute: Box::new(move |project| {
                    drop(sender.send(project.and_then(execute)));
                }),
            })
            .map_err(failure)?;

        receiver.await.map_err(failure)?
    }

    pub(super) async fn control<ValueType: Send + 'static>(
        &self,
        execute: impl FnOnce(&mut State) -> Result<ValueType> + Send + 'static,
    ) -> Result<ValueType> {
        let (sender, receiver) = oneshot::channel();

        self.sender
            .send(Message::Control(Box::new(move |state| {
                drop(sender.send(execute(state)));
            })))
            .map_err(failure)?;

        receiver.await.map_err(failure)?
    }

    pub(super) fn stop(&self) -> eyre::Result<()> {
        if let Some(operation) = self
            .current
            .lock()
            .expect("Internal error: active operation mutex is poisoned.")
            .upgrade()
        {
            operation.cancel();
        }

        drop(self.sender.send(Message::Stop));

        let threads = mem::take(
            &mut *self
                .threads
                .lock()
                .expect("Internal error: worker threads mutex is poisoned."),
        );

        let mut failure = None;

        for thread in threads {
            if let Err(payload) = thread.join() {
                let message = payload
                    .downcast_ref::<&str>()
                    .copied()
                    .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
                    .unwrap_or("Unknown panic payload.");

                failure = Some(eyre::eyre!("Worker thread panicked. {message}"));
            }
        }

        failure.map_or(Ok(()), Err)
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            tracing::error!(%error, "Could not stop the analysis worker.");
        }
    }
}

pub(super) struct Operation {
    requested: AtomicBool,
    native: Mutex<Option<Cancellation>>,
    progress: Option<(Client, ProgressToken, Handle)>,
}

impl Operation {
    pub(super) fn new(progress: Option<(Client, ProgressToken)>) -> Self {
        Self {
            requested: AtomicBool::new(false),
            native: Mutex::new(None),
            progress: progress.map(|(client, token)| (client, token, Handle::current())),
        }
    }

    pub(super) fn requested(&self) -> bool {
        self.requested.load(Ordering::Relaxed)
    }

    pub(super) fn cancel(&self) {
        self.requested.store(true, Ordering::Relaxed);

        if let Some(native) = &*self
            .native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.")
        {
            native.cancel();
        }
    }

    pub(super) fn attach(&self, cancellation: &Cancellation) {
        let mut native = self
            .native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.");

        cancellation.reset();
        *native = Some(cancellation.clone());

        if self.requested() {
            cancellation.cancel();
        }
    }

    pub(super) fn detach(&self) {
        self.native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.")
            .take();
    }

    pub(super) fn report(&self, stage: Progress<'_>) -> bool {
        if self.requested() {
            return false;
        }

        if let Some((client, token, handle)) = &self.progress {
            progress::report(client, token, handle, stage);
        }

        true
    }
}

struct Guard(Option<Arc<Operation>>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(operation) = &self.0 {
            operation.cancel();
        }
    }
}

struct Ticket {
    root: PathBuf,
    generation: u64,
    invalidation: u64,
    documents: Arc<Documents>,
    events: usize,
}

impl Ticket {
    fn capture(state: &State, root: PathBuf) -> Self {
        let project = &state.projects[&root];

        Self {
            root,
            generation: project.generation,
            invalidation: project.invalidation,
            documents: state.documents(),
            events: state.events.len(),
        }
    }

    fn valid(&self, state: &State) -> bool {
        state.projects.get(&self.root).is_some_and(|project| {
            project.generation == self.generation && project.invalidation == self.invalidation
        }) && self.documents == state.documents()
    }

    fn observed(&self, state: &State, feedback: &Feedback) -> bool {
        state.events[self.events..].iter().all(|event| {
            feedback.dependencies.iter().all(|dependency| {
                if !dependency.starts_with(event) && !event.starts_with(dependency) {
                    return true;
                }

                let Some(module) = dependency.to_str() else {
                    return false;
                };

                let Some(observed) = feedback.observed.get(module) else {
                    return false;
                };

                let open = state.projects.values().find_map(|project| {
                    project
                        .documents
                        .get(module)
                        .filter(|document| document.version.is_some())
                });

                if let Some(document) = open {
                    document.text == *observed
                } else {
                    fs::read_to_string(dependency)
                        .is_ok_and(|source| source.as_str() == observed.as_ref())
                }
            })
        })
    }
}

struct Pending {
    ticket: Ticket,
    operation: Arc<Operation>,
}

pub(super) struct Work {
    pub(super) identifier: u64,
    pub(super) snapshot: Snapshot,
    pub(super) history: History,
    pub(super) operation: Arc<Operation>,
    pub(super) action: Action,
}

pub(super) struct Completion {
    pub(super) identifier: u64,
    pub(super) feedback: Option<Feedback>,
    pub(super) success: bool,
    pub(super) deliver: Delivery,
}

pub(super) enum Message {
    Source {
        uri: Uri,
        operation: Option<Arc<Operation>>,
        execute: Source,
    },

    Query {
        uri: Uri,
        operation: Arc<Operation>,
        action: Action,
    },

    Control(Control),
    Complete(Completion),
    Stop,
}

fn run(receiver: &Receiver<Message>, worker: &Sender<Work>) {
    let mut state = State::new();
    let mut pending = BTreeMap::<u64, Pending>::new();
    let mut queued = VecDeque::<Work>::new();
    let mut active = None;
    let mut identifier = 0_u64;

    while let Ok(message) = receiver.recv() {
        let previous = state.documents();

        match message {
            Message::Stop => {
                for pending in pending.values() {
                    pending.operation.cancel();
                }

                return;
            }

            Message::Control(execute) => execute(&mut state),

            Message::Source {
                uri,
                operation,
                execute,
            } => {
                let result = state.select(&uri).and_then(|root| {
                    state.synchronize(&root)?;

                    Ok(root)
                });

                match result {
                    Ok(root) => {
                        let project = state
                            .projects
                            .get_mut(&root)
                            .expect("Internal error: source project is missing.");

                        if let Some(operation) = operation {
                            project
                                .workspace
                                .set_cancellation(move || operation.requested());
                        } else {
                            project.workspace.set_cancellation(|| false);
                        }

                        execute(Ok(project));
                        project.workspace.set_cancellation(|| false);
                    }

                    Err(error) => execute(Err(error)),
                }
            }

            Message::Query {
                uri,
                operation,
                action,
            } => {
                let snapshot = state.select(&uri).and_then(|root| {
                    state.synchronize(&root)?;

                    Ok(state.projects[&root].capture())
                });

                match snapshot {
                    Ok(snapshot) => {
                        let Some(next) = identifier.checked_add(1) else {
                            (action(Err(Error::internal_error())).1)(Ok(()));
                            continue;
                        };

                        identifier = next;
                        let ticket = Ticket::capture(&state, snapshot.root.clone());

                        let work = Work {
                            identifier,
                            snapshot,
                            history: state.history.clone(),
                            operation: Arc::clone(&operation),
                            action,
                        };

                        pending.insert(identifier, Pending { ticket, operation });
                        queued.push_back(work);
                    }

                    Err(error) => (action(Err(error)).1)(Ok(())),
                }
            }

            Message::Complete(completion) => {
                active = None;

                let Some(request) = pending.remove(&completion.identifier) else {
                    (completion.deliver)(Err(Error::request_cancelled()));
                    continue;
                };

                let mut valid = request.ticket.valid(&state);

                if let Some(mut feedback) = completion.feedback {
                    let observed = request.ticket.observed(&state, &feedback);
                    valid &= observed;

                    if let Some(project) = state.projects.get_mut(&request.ticket.root)
                        && project.invalidation == request.ticket.invalidation
                    {
                        if !observed {
                            feedback.observed.clear();

                            let mut dependencies = project.reload_dependencies();
                            dependencies.extend(feedback.reload_dependencies.iter().cloned());
                            let paths = &state.events[request.ticket.events..];

                            if let Err(error) = files::invalidate(project, paths, &dependencies) {
                                tracing::error!(%error, "Could not invalidate changed native dependencies.");

                                match project.changed() {
                                    Ok(()) => project.invalidation = project.generation,

                                    Err(error) => {
                                        tracing::error!(%error, "Could not invalidate changed native dependencies.");
                                    }
                                }
                            }
                        }

                        project.accept(
                            feedback,
                            valid && completion.success && !request.operation.requested(),
                        );
                    }
                }

                (completion.deliver)(if valid {
                    Ok(())
                } else {
                    Err(Error::content_modified())
                });
            }
        }

        if let Err(error) = state.propagate(&previous) {
            tracing::error!(%error, "Could not advance the borrowed document generation.");

            for request in pending.values() {
                request.operation.cancel();
            }
        }

        for request in pending.values() {
            if !request.ticket.valid(&state) {
                request.operation.cancel();
            }
        }

        while active.is_none() {
            let Some(mut work) = queued.pop_front() else {
                break;
            };

            let request = &pending[&work.identifier];
            let valid = request.ticket.valid(&state);

            if !valid || request.operation.requested() {
                pending.remove(&work.identifier);

                let accepted = if valid {
                    Err(Error::request_cancelled())
                } else {
                    Err(Error::content_modified())
                };

                let (_, deliver) = (work.action)(Err(Error::request_cancelled()));
                deliver(accepted);
                continue;
            }

            work.snapshot
                .published
                .clone_from(&state.projects[&work.snapshot.root].published);

            let identifier = work.identifier;

            if let Err(error) = worker.send(work) {
                pending.remove(&identifier);
                ((error.0.action)(Err(failure("Native worker stopped."))).1)(Ok(()));
            } else {
                active = Some(identifier);
            }
        }

        if pending.is_empty() {
            state.events.clear();
        }
    }
}
