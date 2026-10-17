use std::ops::Range;

/// Invalid input syntax or a rejected correction.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    /// Source range involved in the failure.
    pub range: Range<usize>,

    /// Explanation of the rejected input or correction.
    pub message: String,
}
