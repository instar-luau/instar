use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
};

use instar_shared::{
    Completion, EditOutcome, Event, EventPayload, Options, Progress, Reason, SourceSnapshot,
    SourceVersion,
};

use super::request::{ApplyRequest, WriteAuthorization, WriteOutcome};

use crate::{
    change::{ChangeBatch, ProjectChange, SourceChange},
    execution::Runtime,
    inputs::{SourceOrigin, absolute},
    report::ApplyReport,
};

impl Runtime {
    pub(crate) fn apply(
        &self,
        request: ApplyRequest,
        options: &Options,
    ) -> io::Result<ApplyReport> {
        let mut destinations = destinations(request.destinations, &request.edit.proposal.guards)?;

        let mut prepared = Vec::new();

        for version in &request.edit.proposal.guards {
            let original = request
                .edit
                .original
                .source(version.identity)
                .ok_or_else(|| invalid("proposal source is absent"))?;

            let proposed = request
                .edit
                .proposed
                .source(version.identity)
                .ok_or_else(|| invalid("proposed source is absent"))?;

            if original.version != *version {
                return Err(invalid("proposal source guard mismatch"));
            }

            let destination = destinations.remove(version);

            if original.text != proposed.text
                && let Some(destination) = &destination
            {
                let origin = request
                    .edit
                    .original
                    .origin(version.identity)
                    .ok_or_else(|| invalid("proposal source origin is absent"))?;

                if matches!(
                    origin,
                    SourceOrigin::Stdin { .. } | SourceOrigin::Virtual { .. }
                ) || origin.path() != Some(destination.as_path())
                {
                    return Err(invalid(
                        "write destination is not authorized by this source origin",
                    ));
                }
            }

            prepared.push((original, proposed, destination));
        }

        let mut writes = Vec::new();
        let mut revisions = Vec::new();
        let mut completion = Completion::Complete;

        let snapshot = {
            let mut snapshot = self
                .host
                .snapshot
                .write()
                .unwrap_or_else(PoisonError::into_inner);

            if !snapshot.current(&request.edit.inputs) {
                return Err(Reason::StaleQuery.error());
            }

            for (original, proposed, destination) in prepared {
                if let Some(interrupted) = options.interrupted() {
                    writes.push(WriteOutcome::Unapplied {
                        source: original.version,
                        reason: interrupted.error().to_string(),
                    });

                    completion = completion.combine(interrupted);
                    continue;
                }

                if original.text == proposed.text {
                    writes.push(WriteOutcome::Unchanged);
                    continue;
                }

                let Some(destination) = destination else {
                    writes.push(WriteOutcome::NotRequested);
                    continue;
                };

                let outcome = snapshot
                    .transition(ChangeBatch {
                        changes: vec![ProjectChange::Source(SourceChange::Replace {
                            expected: original.version,
                            text: Arc::clone(&proposed.text),
                        })],
                    })
                    .and_then(|next| {
                        let revision = next
                            .source(original.version.identity)
                            .ok_or_else(|| invalid("accepted source is absent"))?
                            .version;

                        write(&destination, original, proposed, options)?;

                        Ok((next, revision))
                    });

                match outcome {
                    Ok((next, revision)) => {
                        *snapshot = next;
                        revisions.push(revision);

                        writes.push(WriteOutcome::Written {
                            source: original.version,
                            revision: revision.revision,
                        });
                    }

                    Err(error) => {
                        completion =
                            completion.combine(options.interrupted().unwrap_or(
                                Completion::Incomplete(vec![Reason::FailedPrerequisite]),
                            ));

                        writes.push(WriteOutcome::Failed {
                            source: original.version,
                            message: error.to_string(),
                        });
                    }
                }
            }

            snapshot.clone()
        };

        for (source, outcome) in request.edit.proposal.guards.iter().zip(&writes) {
            if matches!(
                outcome,
                WriteOutcome::Written { .. } | WriteOutcome::Failed { .. }
            ) {
                options.report(Event {
                    invocation: 0,
                    operation: 0,
                    payload: EventPayload::Progress(Progress {
                        source: Some(*source),
                        phase: "Applying edits".to_owned(),
                        completed: 1,
                        total: Some(1),
                    }),
                });
            }
        }

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::EditOutcome(EditOutcome {
                identity: request.edit.proposal.identity,
                completion: completion.clone(),
            }),
        });

        Ok(ApplyReport {
            identity: request.edit.proposal.identity,
            writes,
            revisions,
            snapshot,
            completion,
        })
    }
}

fn destinations(
    authorizations: Vec<WriteAuthorization>,
    guards: &[SourceVersion],
) -> io::Result<BTreeMap<SourceVersion, PathBuf>> {
    let mut destinations = BTreeMap::new();

    for authorization in authorizations {
        let destination = absolute(&authorization.destination)?;

        if destinations
            .insert(authorization.source, destination)
            .is_some()
        {
            return Err(invalid("duplicate write authorization"));
        }
    }

    if destinations.keys().any(|version| !guards.contains(version)) {
        return Err(invalid(
            "write authorization does not identify a guarded source",
        ));
    }

    Ok(destinations)
}

fn write(
    destination: &Path,
    original: &SourceSnapshot,
    proposed: &SourceSnapshot,
    options: &Options,
) -> io::Result<()> {
    options.check()?;
    let metadata = fs::symlink_metadata(destination)?;

    if !metadata.is_file() {
        return Err(invalid("refusing to replace a nonregular file"));
    }

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)?;

    file.try_lock().map_err(io::Error::other)?;

    if !file.metadata()?.is_file() {
        return Err(invalid("refusing to replace a nonregular file"));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| invalid("write destination has no parent"))?;

    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(proposed.text.as_bytes())?;
    staged.as_file().set_permissions(metadata.permissions())?;
    staged.as_file().sync_all()?;

    let mut bytes = Vec::new();

    (&mut file)
        .take(
            u64::try_from(original.text.len())
                .map_err(io::Error::other)?
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)?;

    options.check()?;

    if bytes != original.text.as_bytes() {
        return Err(invalid("source changed since proposal validation"));
    }

    if !fs::symlink_metadata(destination)?.is_file() {
        return Err(invalid("refusing to replace a nonregular file"));
    }

    drop(file);
    staged.persist(destination).map_err(|error| error.error)?;

    Ok(())
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
