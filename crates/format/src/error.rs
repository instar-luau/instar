use std::{error, fmt, ops::Range};

/// Whether formatting was refused because of the input or an internal failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Invalid source syntax or directives.
    Input,

    /// Rendering or preservation validation failed.
    Internal,
}

/// A parse, directive, or formatting-preservation failure.
#[derive(Debug)]
pub struct Error {
    /// The category used to distinguish findings from operational failures.
    pub kind: Kind,

    /// The affected byte range in the original UTF-8 source.
    pub range: Range<usize>,

    /// The reason formatting was refused.
    pub message: String,
}

impl Error {
    pub(crate) fn input(range: Range<usize>, message: impl Into<String>) -> Self {
        Self {
            kind: Kind::Input,
            range,
            message: message.into(),
        }
    }

    pub(crate) fn internal(range: Range<usize>, message: impl Into<String>) -> Self {
        Self {
            kind: Kind::Internal,
            range,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl error::Error for Error {}
