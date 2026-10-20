use std::{
    mem,
    sync::{
        Arc, Mutex, Weak,
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
};

use instar_bridge::Frontend;

use tokio::{
    sync::{OwnedSemaphorePermit, oneshot},
    task,
};

use tower_lsp_server::{
    gen_lsp_types::{Range as ProtocolRange, TextEdit, Uri},
    jsonrpc::{Error, Result},
};

use super::{
    message::{Action, Delivery, Message},
    native,
    operation::{Guard, Operation},
    source::{self, Ticket},
};

use crate::{editing, error::failure, project::Project, workspace::State};

pub(crate) struct Connection {
    sender: Sender<Message>,
    threads: Mutex<Vec<JoinHandle<()>>>,
    current: Arc<Mutex<Weak<Operation>>>,
}

impl Connection {
    pub(crate) fn new() -> eyre::Result<Self> {
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
            .spawn(move || source::run(&receiver, &worker))
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

    pub(crate) async fn source<ValueType: Send + 'static>(
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
                            drop(sender.send(if pending.is_cancelled() {
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

                    let result = if pending.is_cancelled() {
                        Err(Error::request_cancelled())
                    } else {
                        execute(project)
                    };

                    drop(validated.send(Message::Control(Box::new(move |state| {
                        let is_valid = state.projects.get(&root).is_some_and(|project| {
                            project.generation == generation && project.invalidation == invalidation
                        });

                        drop(sender.send(if !is_valid {
                            Err(Error::content_modified())
                        } else if pending.is_cancelled() {
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

    pub(crate) async fn formatting(
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
                if pending.is_cancelled() {
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
                    .set_cancellation(move || pending.is_cancelled());

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
            let result = if pending.is_cancelled() {
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
                if !ticket.is_valid(state) || state.history.sequence != sequence {
                    Err(Error::content_modified())
                } else if operation.is_cancelled() {
                    Err(Error::request_cancelled())
                } else {
                    result
                }
            })
            .await;

        guard.0 = None;

        result
    }

    pub(crate) async fn query<ValueType: Send + 'static>(
        &self,
        uri: Uri,
        operation: Arc<Operation>,
        execute: impl FnOnce(&mut Project, &mut Frontend<'_>) -> Result<ValueType> + Send + 'static,
    ) -> Result<ValueType> {
        let (sender, receiver) = oneshot::channel();
        let pending = Arc::clone(&operation);
        let mut guard = Guard(Some(Arc::clone(&operation)));

        let action: Action = Box::new(move |session| {
            let result = if pending.is_cancelled() {
                Err(Error::request_cancelled())
            } else {
                session.and_then(|(project, frontend)| execute(project, frontend))
            };

            let is_successful = result.is_ok();

            let deliver: Delivery = Box::new(move |accepted| {
                let result = accepted.and_then(|()| {
                    if pending.is_cancelled() {
                        Err(Error::request_cancelled())
                    } else {
                        result
                    }
                });

                drop(sender.send(result));
            });

            (is_successful, deliver)
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

    pub(crate) async fn update<ValueType: Send + 'static>(
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

    pub(crate) async fn control<ValueType: Send + 'static>(
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

    pub(crate) fn stop(&self) -> eyre::Result<()> {
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
