use std::{io, path::PathBuf};

use crate::format;

/// An asset input, generation, cache, or publication failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A local output could not be accessed.
    #[error("Could not access `{path}`.")]
    Filesystem {
        /// The affected output path.
        path: PathBuf,
        /// The underlying filesystem failure.
        source: io::Error,
    },

    /// The asynchronous runtime could not start.
    #[error("Could not start the asset runtime.")]
    Runtime(#[source] io::Error),

    /// An upstream input could not be downloaded.
    #[error("Could not download `{address}`.")]
    Download {
        /// The requested input URL.
        address: String,
        /// The client, network, or HTTP failure.
        source: reqwest::Error,
    },

    /// An input could not be decoded as JSON.
    #[error("Could not process `{input}` as JSON.")]
    Data {
        /// The input identity.
        input: String,
        /// The underlying JSON failure.
        source: serde_json::Error,
    },

    /// An input does not meet the generation contract.
    #[error("Could not generate assets. {reason}")]
    Invalid {
        /// The invalid input or output condition.
        reason: String,
    },

    /// Native built-in declarations could not be obtained.
    #[error(transparent)]
    Native(#[from] instar_bridge::Error),

    /// A declaration could not be formatted safely.
    #[error(transparent)]
    Format(#[from] format::Error),

    /// An asset operation was cancelled.
    #[error("Asset operation cancelled.")]
    Cancelled,
}
