//! Structured invocation failures and invalid-input error construction.

use std::{
    error::Error,
    fmt::{self, Display},
    io,
};

use serde::{Deserialize, Serialize};

use crate::{Completion, Diagnostic, EditOutcome, SourceOutcome};

/// The boundary at which an invocation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FailureKind {
    /// The invocation's inputs or requested operation are invalid.
    InvalidInvocation,

    /// Bounded admission rejected work because the operation queue is saturated.
    Busy,

    /// An operational prerequisite, worker or transport failed.
    Operational,
}

/// An invocation failure preserving findings and completed work accumulated before failure.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OperationFailure {
    /// Invalid invocation, saturated admission or operational failure classification.
    pub kind: FailureKind,

    /// Failure explanation.
    pub message: String,

    /// Explicit coverage and interruption outcome.
    pub completion: Completion,

    /// Findings retained before failure.
    pub diagnostics: Vec<Diagnostic>,

    /// Source outcomes retained before failure.
    pub completed_sources: Vec<SourceOutcome>,

    /// Edit outcomes retained before failure.
    pub completed_edits: Vec<EditOutcome>,
}

impl OperationFailure {
    /// Retains one combined completion outcome per exact source revision.
    pub fn retain_source(&mut self, outcome: SourceOutcome) {
        if let Some(previous) = self
            .completed_sources
            .iter_mut()
            .find(|previous| previous.source == outcome.source)
        {
            previous.completion = previous.completion.clone().combine(outcome.completion);
        } else {
            self.completed_sources.push(outcome);
        }
    }
}

impl Display for OperationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for OperationFailure {}

impl From<OperationFailure> for io::Error {
    fn from(failure: OperationFailure) -> Self {
        let kind = match &failure.completion {
            Completion::Cancelled => io::ErrorKind::Interrupted,
            Completion::TimedOut => io::ErrorKind::TimedOut,

            Completion::Complete | Completion::Incomplete(_) => match failure.kind {
                FailureKind::InvalidInvocation => io::ErrorKind::InvalidInput,
                FailureKind::Busy => io::ErrorKind::WouldBlock,
                FailureKind::Operational => io::ErrorKind::Other,
            },
        };

        Self::new(kind, failure)
    }
}

/// Creates an invalid-data error while preserving its explanation.
pub fn invalid(error: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
