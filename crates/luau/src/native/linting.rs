use instar_shared::{Availability, Completion, Diagnostic, SourceVersion};
use serde::{Deserialize, Serialize};

use super::revision::AnalysisReference;

/// A preserved upstream Luau warning name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct UpstreamWarningIdentity {
    /// The preserved binding, property or warning name.
    pub name: String,
}

/// The warning context independent of ordinary type reporting.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum LanguageMode {
    /// No ordinary type checking.
    NoCheck,

    /// Nonstrict type checking.
    Nonstrict,

    /// Strict type checking.
    Strict,

    /// Declaration-language checking.
    Definition,
}

/// Explicit upstream warning selections and language mode.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct UpstreamWarningSettings {
    /// Selected upstream warning identities.
    pub enabled: Vec<UpstreamWarningIdentity>,

    /// The warning language mode.
    pub language_mode: LanguageMode,
}

/// Selected upstream warnings for a guarded analysis.
#[derive(Clone, Debug)]
pub struct LintInput {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisReference,

    /// Requested upstream warning coverage.
    pub warnings: UpstreamWarningSettings,
}

/// Coverage for one requested upstream warning.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct WarningAvailability {
    /// The preserved upstream warning identity.
    pub warning: UpstreamWarningIdentity,

    /// Explicit requested warning coverage.
    pub status: Availability,
}

/// An upstream warning identity and detached diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct UpstreamFinding {
    /// The preserved upstream warning identity.
    pub warning: UpstreamWarningIdentity,

    /// The owned revision-anchored warning finding.
    pub diagnostic: Diagnostic,
}

/// Owned warning findings, prerequisites and requested coverage.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LintResult {
    /// The exact captured source snapshot or source revision.
    pub source: SourceVersion,

    /// Owned upstream warning findings.
    pub findings: Vec<UpstreamFinding>,

    /// Findings from actual prerequisite inputs.
    pub prerequisite_diagnostics: Vec<Diagnostic>,

    /// Requested upstream warning coverage.
    pub warnings: Vec<WarningAvailability>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
