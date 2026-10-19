use std::{path::PathBuf, sync::Arc};

use crate::lint::{Error, Symbols};

/// Declaration names or the exact input that prevented their construction.
#[derive(Clone, Debug)]
pub enum SymbolOutcome {
    /// The selected declaration environment is valid.
    Ready(Arc<Symbols>),

    /// A declaration source contains invalid syntax.
    Invalid(InvalidSource),
}

/// A declaration failure with its original source snapshot.
#[derive(Clone, Debug)]
pub struct InvalidSource {
    /// The source that could not be parsed.
    pub origin: Origin,

    /// Exact declaration bytes used during symbol collection.
    pub text: Arc<str>,

    /// The declaration syntax failure, relative to the retained text.
    pub error: Error,
}

/// The origin of a declaration source.
#[derive(Clone, Debug)]
pub enum Origin {
    /// A declaration file.
    File(PathBuf),

    /// Native in-memory builtin declarations.
    Builtins,
}
