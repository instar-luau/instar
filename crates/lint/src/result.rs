use instar_shared::{Availability, Completion, Diagnostic};
use serde::Serialize;

use crate::{RuleFix, RuleIdentity};

/// Coverage established by one selected rule.
#[derive(Clone, Debug, Serialize)]
pub struct RuleAvailability {
    /// Catalog identity whose coverage is described.
    pub rule: RuleIdentity,

    /// Established coverage or its explicit missing prerequisite.
    pub status: Availability,
}

/// Findings and proposals retained even when requested coverage is incomplete.
#[derive(Clone, Debug)]
pub struct RuleResult {
    /// Revision-anchored findings and prerequisite diagnostics.
    pub findings: Vec<Diagnostic>,

    /// Guarded correction proposals without write authority.
    pub fixes: Vec<RuleFix>,

    /// Coverage of each selected Instar producer.
    pub availability: Vec<RuleAvailability>,

    /// Completion independent of finding severity.
    pub completion: Completion,
}

impl Default for RuleResult {
    fn default() -> Self {
        Self {
            findings: Vec::new(),
            fixes: Vec::new(),
            availability: Vec::new(),
            completion: Completion::Complete,
        }
    }
}

impl RuleResult {
    /// Retains another producer's findings, proposals, coverage, and completion.
    pub fn combine(&mut self, mut other: Self) {
        self.findings.append(&mut other.findings);
        self.fixes.append(&mut other.fixes);
        self.availability.append(&mut other.availability);
        self.completion = self.completion.clone().combine(other.completion);
    }
}
