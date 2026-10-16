use std::{error, fmt, ops::Range};

use instar_configuration::linting::Severity;

/// A byte-range replacement guarded by its original source text.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// Invalid input syntax or a rejected correction.
#[derive(Debug)]
pub struct Error {
    /// Source range involved in the failure.
    pub range: Range<usize>,

    /// Explanation of the rejected input or correction.
    pub message: String,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {}
