//! Luau type checking.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File selection for checking; native language settings belong to Luau configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Include patterns; omission inherits and an empty list selects every source.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude patterns; omission inherits and an empty list clears exclusions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
}

/// Source location anchored to an immutable host module revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location<Module> {
    /// Exact host-owned module identity.
    pub module: Module,

    /// Source revision for the byte range.
    pub revision: u64,

    /// UTF-8 byte range with an exclusive end.
    pub range: [usize; 2],
}

/// Checker diagnostic category, retaining upstream type error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Native parser error.
    Syntax {
        /// Upstream `TypeError::code()`.
        code: i32,
    },

    /// Native type error.
    Type {
        /// Upstream `TypeError::code()`.
        code: i32,
    },

    /// Import resolution error.
    Resolution {
        /// Upstream code when a native diagnostic exists.
        code: Option<i32>,
    },

    /// Native analysis failure or complexity limit.
    Analysis {
        /// Upstream code when available.
        code: Option<i32>,
    },

    /// Required native environment capability is unavailable.
    Unsupported,
}

impl Kind {
    /// Returns the unmodified upstream diagnostic code, when present.
    #[must_use]
    pub const fn native_code(self) -> Option<i32> {
        match self {
            Self::Syntax { code } | Self::Type { code } => Some(code),
            Self::Resolution { code } | Self::Analysis { code } => code,
            Self::Unsupported => None,
        }
    }
}

/// Additional source context supplied by a native diagnostic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Related<Module> {
    /// Related source location.
    pub location: Location<Module>,

    /// Native explanation of the relationship.
    pub message: String,
}

/// A structured checker error; native lint warnings belong to the linter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic<Module> {
    /// Actual source location, including dependency modules.
    pub location: Location<Module>,

    /// Diagnostic category and native identity.
    pub kind: Kind,

    /// Native explanation or host resolution failure.
    pub message: String,

    /// Related locations retained from structured native error data.
    pub related: Vec<Related<Module>>,
}

/// Why analysis could not produce a complete answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The caller requested cancellation.
    Cancelled,

    /// The analysis budget expired.
    Timeout,

    /// Required environment support is unavailable.
    Unsupported,

    /// Declarations could not establish a valid environment.
    Environment,

    /// Native analysis exceeded a complexity limit or failed internally.
    Analysis,
}

/// Whether the result covers all selected entries and their dependencies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Completion {
    /// Analysis finished; diagnostics may still contain errors.
    Complete,

    /// Partial diagnostics must not be interpreted as success.
    Incomplete(Reason),
}

/// Checker output, independent of host identities and native storage.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Result<Module> {
    /// Modules in the selected reachable graph, once per exact identity.
    pub modules: Vec<Module>,

    /// Errors reported once at their actual source revisions.
    pub diagnostics: Vec<Diagnostic<Module>>,

    /// Explicit completeness of this analysis.
    pub completion: Completion,
}

/// A clonable cancellation signal that can be raised from another thread.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl Cancellation {
    /// Requests cancellation of analyses using this signal.
    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }
}

/// Caller-supplied limits for one project analysis.
#[derive(Clone, Debug)]
pub struct Options {
    /// Total elapsed budget, including host preparation and declarations.
    ///
    /// Native module typechecking is cooperatively interrupted. Upstream declaration
    /// loading and parsing have no interruptible API; their budget is checked before
    /// and after execution, and overruns are reported as incomplete.
    pub timeout: std::time::Duration,

    /// Shared cancellation signal.
    pub cancellation: Cancellation,
}

impl Options {
    /// Creates limits with a fresh cancellation signal.
    #[must_use]
    pub fn new(timeout: std::time::Duration) -> Self {
        Self {
            timeout,
            cancellation: Cancellation::default(),
        }
    }
}
