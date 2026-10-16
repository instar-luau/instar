use std::error;
use std::fmt;

/// A host callback or native frontend failure.
#[derive(Debug)]
pub enum Error {
    /// An operational failure reported by the host.
    Host(HostError),

    /// A failure raised by native Luau analysis.
    Native {
        /// The native failure message.
        reason: String,
    },
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(source) => fmt::Display::fmt(source, formatter),
            Self::Native { reason } => formatter.write_str(reason),
        }
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        match self {
            Self::Host(source) => Some(source),
            Self::Native { .. } => None,
        }
    }
}

/// An operational host failure with its module and callback context.
#[derive(Debug)]
pub struct HostError {
    /// The host module identifier involved in the failure.
    pub module: String,

    /// The callback operation that failed.
    pub operation: Operation,

    /// The reason the operation failed.
    pub reason: String,
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            module,
            operation,
            reason,
        } = self;

        write!(
            formatter,
            "cannot {operation} for module {module}: {reason}"
        )
    }
}

impl error::Error for HostError {}

/// A host callback operation.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    /// Load module source text.
    Source,

    /// Find a parent navigation context.
    Parent,

    /// Find a child navigation context.
    Child,

    /// Resolve an alias.
    Alias,

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
            Self::Alias => "resolve alias",
            Self::Configuration => "resolve configuration",
            Self::Environment => "load environment",
            Self::Flags => "resolve flags",
            Self::Hierarchy => "load hierarchy",
        })
    }
}
