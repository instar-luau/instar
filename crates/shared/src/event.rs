use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{Completion, Diagnostic, EditIdentity, SourceVersion};

/// A streamed event with invocation and operation identities reused by terminal reports.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Event {
    /// Invocation identity; project observers map physical worker identities to logical invocations.
    pub invocation: u64,

    /// Operation identity within the invocation.
    pub operation: u64,

    /// Progress, findings, completed outcomes or operational logging.
    pub payload: EventPayload,
}

/// The distinct event responsibilities published during an operation.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum EventPayload {
    /// Completed work at a measured phase boundary.
    Progress(Progress),

    /// A filesystem path consulted by the operation, including absent inputs.
    InputPath(PathBuf),

    /// A finding retaining its stable identity and exact input revisions.
    Diagnostic(Diagnostic),

    /// An authoritative source completion outcome.
    SourceOutcome(SourceOutcome),

    /// An authoritative edit completion outcome.
    EditOutcome(EditOutcome),

    /// Operational logging, independent of diagnostics.
    Log(OperationalLog),
}

/// Completed work at a named phase boundary.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Progress {
    /// Phase whose work is measured.
    pub phase: String,

    /// Exact source revision associated with this work, when applicable.
    pub source: Option<SourceVersion>,

    /// Completed work item count.
    pub completed: usize,

    /// Total work item count when discovery has established it.
    pub total: Option<usize>,
}

/// Completed source work retaining its exact source identity and revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceOutcome {
    /// Exact original input revision.
    pub source: SourceVersion,

    /// Coverage and interruption outcome for this source.
    pub completion: Completion,
}

/// Completed edit work retaining the proposal's identity.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EditOutcome {
    /// Proposal identity shared with its terminal report.
    pub identity: EditIdentity,

    /// Coverage and interruption outcome for this proposal.
    pub completion: Completion,
}

/// Operational log severity, separate from source finding severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum LogLevel {
    /// Fine-grained operational tracing.
    Trace,

    /// Operational debugging information.
    Debug,

    /// Informational operational state.
    Information,

    /// A recoverable operational warning.
    Warning,

    /// An operational error.
    Error,
}

/// Operational logging that does not represent a source diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OperationalLog {
    /// Operational log severity.
    pub level: LogLevel,

    /// Log explanation.
    pub message: String,
}
