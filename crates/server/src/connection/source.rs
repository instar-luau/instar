use std::{
    collections::{BTreeMap, VecDeque},
    fs,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{Receiver, Sender},
    },
};

use tokio_util::sync::CancellationToken;
use tower_lsp_server::jsonrpc::Error;

use super::{
    message::{Message, Target, Work},
    operation::{Consistency, Operation, Priority},
};

use crate::{
    error::reason,
    files,
    project::Feedback,
    workspace::{Documents, State},
};

pub(super) struct Ticket {
    root: PathBuf,
    generation: u64,
    invalidation: u64,
    documents: Arc<Documents>,
    events: usize,
}

impl Ticket {
    pub(super) fn capture(state: &State, root: PathBuf) -> Self {
        let project = &state.projects[&root];

        Self {
            root,
            generation: project.generation,
            invalidation: project.invalidation,
            documents: state.documents(),
            events: state.events.len(),
        }
    }

    pub(super) fn is_valid(&self, state: &State) -> bool {
        state.projects.get(&self.root).is_some_and(|project| {
            project.generation == self.generation && project.invalidation == self.invalidation
        }) && self.documents == state.documents()
    }

    fn accepts(&self, state: &State, consistency: Consistency) -> bool {
        match consistency {
            Consistency::Snapshot => state
                .projects
                .get(&self.root)
                .is_some_and(|project| project.invalidation == self.invalidation),

            Consistency::Current => self.is_valid(state),
        }
    }

    fn has_current_observations(&self, state: &State, feedback: &Feedback) -> bool {
        state.events[self.events..].iter().all(|event| {
            feedback.observations.dependencies.iter().all(|dependency| {
                if !dependency.starts_with(event) && !event.starts_with(dependency) {
                    return true;
                }

                let Some(module) = dependency.to_str() else {
                    return false;
                };

                let Some(observed) = feedback.observations.sources.get(module) else {
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

pub(super) fn run(receiver: &Receiver<Message>, worker: &Sender<Work>) {
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
                                .set_cancellation(operation.cancellation.clone());
                        } else {
                            project.workspace.set_cancellation(CancellationToken::new());
                        }

                        execute(Ok(project));
                        project.workspace.set_cancellation(CancellationToken::new());
                    }

                    Err(error) => execute(Err(error)),
                }
            }

            Message::Query {
                target,
                operation,
                action,
            } => {
                let snapshot = match target {
                    Target::Uri(uri) => state.select(&uri).and_then(|root| {
                        state.synchronize(&root)?;

                        Ok(state.projects[&root].capture())
                    }),

                    Target::Snapshot(snapshot) => {
                        if state.projects.contains_key(&snapshot.root) {
                            Ok(*snapshot)
                        } else {
                            Err(Error::content_modified())
                        }
                    }
                };

                match snapshot {
                    Ok(snapshot) => {
                        let Some(next) = identifier.checked_add(1) else {
                            let (_, deliver) = action(Err(Error::internal_error()));
                            deliver(Ok(()));
                            continue;
                        };

                        identifier = next;

                        let ticket = Ticket {
                            generation: snapshot.generation,
                            invalidation: snapshot.invalidation,
                            ..Ticket::capture(&state, snapshot.root.clone())
                        };

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

                    Err(error) => {
                        let (_, deliver) = action(Err(error));
                        deliver(Ok(()));
                    }
                }
            }

            Message::Complete(completion) => {
                active = None;

                let Some(request) = pending.remove(&completion.identifier) else {
                    (completion.deliver)(Err(Error::request_cancelled()));
                    continue;
                };

                let is_current = request.ticket.is_valid(&state);

                let mut is_valid = request
                    .ticket
                    .accepts(&state, request.operation.consistency);

                if let Some(feedback) = completion.feedback {
                    let has_current_observations =
                        request.ticket.has_current_observations(&state, &feedback);

                    is_valid &= has_current_observations;

                    if let Some(project) = state.projects.get_mut(&request.ticket.root)
                        && project.invalidation == request.ticket.invalidation
                    {
                        if !has_current_observations {
                            let mut dependencies = project.reload_dependencies();

                            dependencies
                                .extend(feedback.observations.reload_dependencies.iter().cloned());

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
                            has_current_observations,
                            is_current
                                && is_valid
                                && completion.is_successful
                                && !request.operation.is_cancelled(),
                        );
                    }
                }

                (completion.deliver)(if is_valid {
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
            if !request
                .ticket
                .accepts(&state, request.operation.consistency)
            {
                request.operation.cancel();
            }
        }

        while active.is_none() {
            let position = queued
                .iter()
                .position(|work| work.operation.priority == Priority::Interactive)
                .unwrap_or(0);

            let Some(mut work) = queued.remove(position) else {
                break;
            };

            let request = &pending[&work.identifier];

            let is_valid = request
                .ticket
                .accepts(&state, request.operation.consistency);

            if !is_valid || request.operation.is_cancelled() {
                pending.remove(&work.identifier);

                let accepted = if is_valid {
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
                let (_, deliver) = (error.0.action)(Err(reason("Native worker stopped.")));
                deliver(Ok(()));
            } else {
                active = Some(identifier);
            }
        }

        if pending.is_empty() {
            state.events.clear();
        }
    }
}
