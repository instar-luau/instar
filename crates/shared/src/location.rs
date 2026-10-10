use serde::{Deserialize, Serialize};

/// Source location anchored to an immutable host module revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Location<Module> {
    /// Exact host-owned module identity.
    pub module: Module,

    /// Source revision for the byte range.
    pub revision: u64,

    /// UTF-8 byte range with an exclusive end.
    pub range: [usize; 2],
}

/// Additional source context supplied by a native diagnostic.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Related<Module> {
    /// Related source location.
    pub location: Location<Module>,

    /// Native explanation of the relationship.
    pub message: String,
}
