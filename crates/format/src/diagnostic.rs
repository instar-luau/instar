use serde::{Deserialize, Serialize};

/// A formatter diagnostic anchored to source bytes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Diagnostic {
    /// Exclusive source byte range.
    pub range: [usize; 2],

    /// Explanation of the failure.
    pub message: String,
}
