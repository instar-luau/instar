use std::{io, path::PathBuf, sync::Arc};

use crate::assets;

/// A workspace filesystem, path, or selection failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("Could not access `{}`.", path.display())]
    Filesystem {
        /// The path involved in the operation.
        path: PathBuf,
        /// The underlying filesystem failure.
        source: io::Error,
    },

    /// A workspace operation was cancelled.
    #[error("Workspace operation cancelled.")]
    Cancelled,

    /// A workspace asynchronous runtime could not start.
    #[error("Could not start the workspace runtime.")]
    Runtime(#[source] io::Error),

    /// An asset operation failed.
    #[error(transparent)]
    Assets(assets::Error),

    /// A sourcemap generator could not run successfully.
    #[error("Could not generate a sourcemap with `{executable}`.{details}")]
    Generate {
        /// The generator executable.
        executable: String,
        /// The process or output failure.
        source: io::Error,
        /// Additional generator diagnostics.
        details: String,
    },

    /// A generator returned an invalid sourcemap.
    #[error("Invalid sourcemap from `{executable}`.")]
    GeneratedData {
        /// The generator executable.
        executable: String,
        /// The JSON decoding failure.
        source: serde_json::Error,
    },

    /// A shared sourcemap producer failed.
    #[error(transparent)]
    Generation(Arc<Self>),

    /// A directory traversal failed.
    #[error("Could not traverse the workspace.")]
    Traversal(#[source] ignore::Error),

    /// A selection pattern could not be compiled.
    #[error("Invalid selection pattern.")]
    Pattern(#[source] globset::Error),

    /// Configuration could not be loaded or deserialized.
    #[error("Invalid configuration.")]
    Configuration(#[source] Box<figment::Error>),

    /// Native language configuration could not be parsed or evaluated.
    #[error("Invalid language configuration `{}`.", path.display())]
    Language {
        /// The configuration file.
        path: PathBuf,
        /// The native parsing or execution failure.
        source: instar_bridge::Error,
    },

    /// Native environment settings could not be validated.
    #[error("Could not prepare analysis.")]
    Analysis(#[source] instar_bridge::Error),

    /// Environment metadata or documentation could not be decoded.
    #[error("Could not decode `{}` as JSON.", path.display())]
    Data {
        /// The input file.
        path: PathBuf,
        /// The JSON decoding failure.
        source: serde_json::Error,
    },

    /// A path cannot identify a workspace input.
    #[error("Cannot use `{}`. {reason}", path.display())]
    Path {
        /// The rejected path.
        path: PathBuf,
        /// The reason the path cannot be used.
        reason: &'static str,
    },
}

impl From<assets::Error> for Error {
    fn from(error: assets::Error) -> Self {
        match error {
            assets::Error::Cancelled => Self::Cancelled,
            assets::Error::Filesystem { path, source } => Self::Filesystem { path, source },
            assets::Error::Runtime(source) => Self::Runtime(source),
            error => Self::Assets(error),
        }
    }
}
