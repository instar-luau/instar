use std::ops::Range;

/// A replacement guarded by the original UTF-8 source bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    /// The byte range to replace in the original document.
    pub range: Range<usize>,

    /// The source text that must still occupy the replacement range.
    pub expected: String,

    /// The replacement text for the guarded range.
    pub replacement: String,
}
