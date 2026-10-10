//! Immutable source inputs, revision-anchored findings and shared execution contracts.

pub mod error;
pub mod process;

mod diagnostic;
mod edit;
mod event;
mod execution;
mod location;
mod source;

pub use diagnostic::{
    Diagnostic, DiagnosticCategory, DiagnosticTag, FindingIdentity, RelatedLocation, Severity,
};

pub use edit::{EditIdentity, EditIntent, EditProposal, TextEdit};
pub use error::{FailureKind, OperationFailure};

pub use event::{
    EditOutcome, Event, EventPayload, LogLevel, OperationalLog, Progress, SourceOutcome,
};

pub use execution::{
    Availability, Cancellation, CancellationGuard, Completion, Knowledge, Options, Reason,
};

pub use location::{Location, Span};

pub use source::{
    ContextIdentity, LineIndex, LinePosition, SourceEncoding, SourceIdentity, SourceSnapshot,
    SourceVersion,
};
