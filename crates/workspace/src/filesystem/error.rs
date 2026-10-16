use std::{error, fmt, io, path::PathBuf};

/// A workspace filesystem, path, or selection failure.
#[derive(Debug)]
pub enum Error {
    /// A filesystem operation failed.
    Filesystem {
        /// The path involved in the operation.
        path: PathBuf,
        /// The underlying filesystem failure.
        source: io::Error,
    },

    /// A directory traversal failed.
    Traversal(ignore::Error),

    /// A selection pattern could not be compiled.
    Pattern(globset::Error),

    /// Configuration could not be loaded or deserialized.
    Configuration(Box<figment::Error>),

    /// Environment metadata or documentation could not be decoded.
    Data {
        /// The input file.
        path: PathBuf,
        /// The JSON decoding failure.
        source: serde_json::Error,
    },

    /// A path cannot identify a workspace input.
    Path {
        /// The rejected path.
        path: PathBuf,
        /// The reason the path cannot be used.
        reason: &'static str,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Filesystem { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Traversal(source) => fmt::Display::fmt(source, formatter),
            Self::Pattern(source) => fmt::Display::fmt(source, formatter),
            Self::Configuration(source) => fmt::Display::fmt(source, formatter),
            Self::Data { path, source } => write!(formatter, "{}: {source}", path.display()),
            Self::Path { path, reason } => write!(formatter, "{}: {reason}", path.display()),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Filesystem { source, .. } => Some(source),
            Self::Traversal(source) => Some(source),
            Self::Pattern(source) => Some(source),
            Self::Configuration(source) => Some(source),
            Self::Data { source, .. } => Some(source),
            Self::Path { .. } => None,
        }
    }
}
