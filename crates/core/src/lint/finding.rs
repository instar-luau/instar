use std::ops::Range;

use crate::settings::lint::Severity;

/// A byte-range replacement guarded by its original source text.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    /// Half-open UTF-8 byte range in the original source.
    pub range: Range<usize>,

    /// Required original contents of the range.
    pub expected: String,

    /// Replacement source text.
    pub replacement: String,
}

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
