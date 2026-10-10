use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
};

use instar_shared::{
    Completion, EditOutcome, Event, EventPayload, Options, Reason, SourceSnapshot, SourceVersion,
};

use super::request::{ApplyRequest, WriteAuthorization, WriteOutcome};

use crate::{
    execution::Runtime,
    inputs::{SourceOrigin, absolute},
    report::ApplyReport,
};

impl Runtime {
    pub(crate) fn apply(
        &mut self,
        request: ApplyRequest,
        options: &Options,
    ) -> io::Result<ApplyReport> {
        let mut destinations = destinations(request.destinations, &request.edit.proposal.guards)?;

        let snapshot = self.host.snapshot();

        if !snapshot.current(&request.edit.inputs) {
            return Err(instar_shared::Reason::StaleQuery.error());
        }

        let mut writes = Vec::new();
        let mut completion = Completion::Complete;

        for version in &request.edit.proposal.guards {
            let destination = destinations.remove(version);

            if let Some(interrupted) = options.interrupted() {
                writes.push(WriteOutcome::Unapplied {
                    source: *version,
                    reason: interrupted.error().to_string(),
                });

                completion = completion.combine(interrupted);
                continue;
            }

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

            if original.text == proposed.text {
                writes.push(WriteOutcome::Unchanged);
                continue;
            }

            let Some(destination) = destination else {
                writes.push(WriteOutcome::NotRequested);
                continue;
            };

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

            let outcome = compare(&destination, original, options);

            match outcome {
                Ok(outcome) => {
                    completion = completion
                        .combine(Completion::Incomplete(vec![Reason::UnsupportedFeature]));

                    writes.push(outcome);
                }

                Err(error) => {
                    completion = completion.combine(
                        options
                            .interrupted()
                            .unwrap_or(Completion::Incomplete(vec![Reason::FailedPrerequisite])),
                    );

                    writes.push(WriteOutcome::Failed {
                        source: *version,
                        message: error.to_string(),
                    });
                }
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
            revisions: Vec::new(),
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

fn compare(
    destination: &Path,
    original: &SourceSnapshot,
    options: &Options,
) -> io::Result<WriteOutcome> {
    options.check()?;
    let metadata = fs::symlink_metadata(destination)?;

    if !metadata.is_file() {
        return Err(invalid("refusing to replace a nonregular file"));
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(destination)?;

    file.try_lock().map_err(io::Error::other)?;
    let mut bytes = Vec::new();

    file.take(
        u64::try_from(original.text.len())
            .map_err(io::Error::other)?
            .saturating_add(1),
    )
    .read_to_end(&mut bytes)?;

    options.check()?;

    if bytes != original.text.as_bytes() {
        return Err(invalid("source changed since proposal validation"));
    }

    Ok(WriteOutcome::Unapplied { source: original.version, reason: "comparison-through-commit protection against noncooperating external writers is unavailable; atomic replacement alone is not compare-and-swap".to_owned() })
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
