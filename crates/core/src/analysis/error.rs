use std::ops::Range;

/// Invalid source syntax.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    /// Source byte range involved in the failure.
    pub range: Range<usize>,

    /// Explanation of the rejected syntax.
    pub message: String,
}
