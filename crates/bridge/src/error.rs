use std::{error::Error as Failure, fmt};

/// A host callback or native frontend failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// An operational failure reported by the host.
    #[error(transparent)]
    Host(HostError),

    /// An incompatible frontend temporarily owns the process-wide native flags.
    #[error("Different Luau flag settings are already in use by another frontend.")]
    FlagsBusy,

    /// A failure raised by native Luau analysis.
    #[error("{reason}")]
    Native {
        /// The native failure message.
        reason: String,
    },
}

/// An operational host failure with its module and callback context.
#[derive(Debug, thiserror::Error)]
#[error("Could not {operation} for module `{module}`.")]
pub struct HostError {
    /// The host module identifier involved in the failure.
    pub module: String,

    /// The callback operation that failed.
    pub operation: Operation,

    /// The underlying host failure.
    pub source: Box<dyn Failure + Send + Sync>,
}

/// A host callback operation.
#[derive(Copy, Clone, Debug, Eq, Hash, PartialEq)]
pub enum Operation {
    /// Load module source text.
    Source,

    /// Find a parent navigation context.
    Parent,

    /// Find a child navigation context.
    Child,

    /// List child navigation contexts.
    Children,

    /// Resolve an alias.
    Alias,

    /// List aliases in a module's scope.
    Aliases,

    /// Resolve type-checking configuration.
    Configuration,

    /// Load declarations and capabilities for a named environment.
    Environment,

    /// Resolve frontend-wide native flags.
    Flags,

    /// Load an instance hierarchy.
    Hierarchy,
}

impl fmt::Display for Operation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Source => "read source",
            Self::Parent => "resolve parent",
            Self::Child => "resolve child",
            Self::Children => "list children",
            Self::Alias => "resolve alias",
            Self::Aliases => "list aliases",
            Self::Configuration => "resolve configuration",
            Self::Environment => "load environment",
            Self::Flags => "resolve flags",
            Self::Hierarchy => "load hierarchy",
        })
    }
}
