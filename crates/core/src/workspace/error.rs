use std::{io, path::PathBuf, sync::Arc};

use crate::lint;

/// A workspace filesystem, path, or selection failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("cannot access `{}`", path.display())]
    Filesystem {
        /// The path involved in the operation.
        path: PathBuf,
        /// The underlying filesystem failure.
        source: io::Error,
    },

    /// A workspace operation was cancelled.
    #[error("workspace operation cancelled")]
    Cancelled,

    /// A workspace asynchronous runtime could not start.
    #[error("cannot start workspace runtime")]
    Runtime(#[source] io::Error),

    /// An asset could not be downloaded.
    #[error("cannot download `{url}`")]
    Download {
        /// The asset URL.
        url: String,
        /// The client, network, or HTTP failure.
        source: reqwest::Error,
    },

    /// A sourcemap generator could not run successfully.
    #[error("cannot generate sourcemap with `{executable}`")]
    Generate {
        /// The generator executable.
        executable: String,
        /// The process or output failure.
        source: io::Error,
    },

    /// A generator returned an invalid sourcemap.
    #[error("invalid sourcemap from `{executable}`")]
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
    #[error("cannot traverse workspace")]
    Traversal(#[source] ignore::Error),

    /// A selection pattern could not be compiled.
    #[error("invalid selection pattern")]
    Pattern(#[source] globset::Error),

    /// Configuration could not be loaded or deserialized.
    #[error("invalid configuration")]
    Configuration(#[source] Box<figment::Error>),

    /// Native environment settings could not be validated.
    #[error("cannot prepare analysis")]
    Analysis(#[source] instar_luau::Error),

    /// A declaration source contains invalid syntax.
    #[error("cannot parse `{}`", path.display())]
    Syntax {
        /// The declaration source.
        path: PathBuf,
        /// The syntax failure.
        source: lint::Error,
    },

    /// Environment metadata or documentation could not be decoded.
    #[error("cannot decode `{}`", path.display())]
    Data {
        /// The input file.
        path: PathBuf,
        /// The JSON decoding failure.
        source: serde_json::Error,
    },

    /// A path cannot identify a workspace input.
    #[error("cannot use `{}`: {reason}", path.display())]
    Path {
        /// The rejected path.
        path: PathBuf,
        /// The reason the path cannot be used.
        reason: &'static str,
    },
}
