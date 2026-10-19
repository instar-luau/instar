use std::ops::Range;

/// Invalid input syntax or a rejected correction.
#[derive(Clone, Debug, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    /// Source range involved in the failure.
    pub range: Range<usize>,

    /// Explanation of the rejected input or correction.
    pub message: String,
}

/// Invalid source syntax or lint directives.
#[derive(Clone, Debug, thiserror::Error)]
pub enum Failure {
    /// Strict source parsing failed.
    #[error(transparent)]
    Syntax(Error),

    /// A suppression directive was rejected.
    #[error(transparent)]
    Directive(Error),
}
