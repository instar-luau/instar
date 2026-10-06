/// A formatter diagnostic anchored to source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Exclusive source byte range.
    pub range: [usize; 2],

    /// Explanation of the failure.
    pub message: String,
}
