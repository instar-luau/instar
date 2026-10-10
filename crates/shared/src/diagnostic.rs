use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

use crate::{Location, SourceVersion};

/// A stable finding key derived from its producer, exact inputs, code and location.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct FindingIdentity {
    producer: String,
    inputs: Vec<SourceVersion>,
    code: String,
    location: Location,
}

impl FindingIdentity {
    /// Constructs a full-key identity with canonicalized inputs including its location source.
    #[must_use]
    pub fn new(
        producer: impl Into<String>,
        mut inputs: Vec<SourceVersion>,
        code: impl Into<String>,
        location: Location,
    ) -> Self {
        inputs.push(location.source);
        inputs.sort_unstable();
        inputs.dedup();

        Self {
            producer: producer.into(),
            inputs,
            code: code.into(),
            location,
        }
    }

    /// Returns the producer's stable name.
    #[must_use]
    pub fn producer(&self) -> &str {
        &self.producer
    }

    /// Returns the exact canonicalized input revisions.
    #[must_use]
    pub fn inputs(&self) -> &[SourceVersion] {
        &self.inputs
    }

    /// Returns the rule or upstream diagnostic code independent of presentation.
    #[must_use]
    pub fn code(&self) -> &str {
        &self.code
    }

    /// Returns the finding's exact primary location.
    #[must_use]
    pub const fn location(&self) -> Location {
        self.location
    }
}

/// Diagnostic classification independent of its code and reporting severity.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema,
)]
pub enum DiagnosticCategory {
    /// A source grammar diagnostic.
    Syntax,

    /// A native type diagnostic.
    Type,

    /// An import or source resolution diagnostic.
    Resolution,

    /// An analysis preparation or coverage diagnostic.
    Analysis,

    /// A configuration source diagnostic.
    Configuration,

    /// An Instar or upstream lint rule finding.
    Lint,

    /// A formatting validation diagnostic.
    Formatting,

    /// An editor transformation diagnostic.
    Edit,
}

/// Reporting severity, separate from rule codes and analysis requirements.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
pub enum Severity {
    /// A reported error.
    Error,

    /// A reported warning.
    Warning,

    /// An informational finding.
    Information,

    /// A presentation hint.
    Hint,
}

/// Structured presentation metadata associated with a finding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum DiagnosticTag {
    /// The highlighted source is unnecessary.
    Unnecessary,

    /// The highlighted symbol or construct is deprecated.
    Deprecated,
}

/// Additional exact source context explaining a diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct RelatedLocation {
    /// Exact source revision and byte span.
    pub location: Location,

    /// Explanation of the relationship to the primary finding.
    pub message: String,
}

/// A concrete revision-anchored finding with stable identity and reporting metadata.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Diagnostic {
    /// Stable full-key identity reused by events and terminal reports.
    pub identity: FindingIdentity,

    /// Rule or original upstream code, independent of severity.
    pub code: String,

    /// Classification independent of the code.
    pub category: DiagnosticCategory,

    /// Effective reporting severity.
    pub severity: Severity,

    /// Finding explanation.
    pub message: String,

    /// Exact primary source revision and range.
    pub primary: Location,

    /// Related source contexts retained from the producer.
    pub related: Vec<RelatedLocation>,

    /// Structured presentation metadata.
    pub tags: Vec<DiagnosticTag>,
}
