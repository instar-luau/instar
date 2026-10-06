use crate::{Location, Related};

/// Analysis diagnostic category, retaining upstream type error codes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Source parser error.
    Syntax {
        /// Upstream `TypeError::code()` when reported by native analysis.
        code: Option<i32>,
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
            Self::Type { code } => Some(code),
            Self::Syntax { code } | Self::Resolution { code } | Self::Analysis { code } => code,
            Self::Unsupported => None,
        }
    }
}

/// A structured native analysis error.
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
