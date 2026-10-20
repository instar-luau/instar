use std::ops::Range;

use crate::{edit::Edit, settings::lint::Severity};

/// A configured finding and its validated correction proposals.
#[derive(Clone, Debug)]
pub struct Finding {
    /// Canonical lint rule name.
    pub rule: String,

    /// Configured diagnostic severity.
    pub severity: Severity,

    /// Half-open source byte range.
    pub range: Range<usize>,

    /// Human-readable diagnostic.
    pub message: String,

    /// Source-guarded corrections, if supported by the rule.
    pub edits: Vec<Edit>,
}
