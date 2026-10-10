use serde::Serialize;

use super::{analysis::AnalysisPurpose, semantics::AnalysisIdentity};

/// An opaque identity guarding worker-local established analysis state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnalysisReference {
    pub(crate) analysis: AnalysisIdentity,
    pub(crate) client: u64,
    pub(crate) generation: u64,
    pub(crate) frontend_revision: u64,
    pub(crate) purpose: AnalysisPurpose,
}

impl AnalysisReference {
    /// Returns the referenced logical analysis identity.
    #[must_use]
    pub const fn identity(&self) -> &AnalysisIdentity {
        &self.analysis
    }

    /// Returns the established source or editor cache purpose.
    #[must_use]
    pub const fn purpose(&self) -> AnalysisPurpose {
        self.purpose
    }

    /// Returns the guarded worker-local Frontend revision.
    #[must_use]
    pub const fn frontend_revision(&self) -> u64 {
        self.frontend_revision
    }
}
