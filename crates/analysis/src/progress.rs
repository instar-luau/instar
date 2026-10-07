use serde::{Deserialize, Serialize};

/// Completed work at a named analysis phase boundary.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Progress {
    /// Phase whose work is being measured.
    pub phase: String,

    /// Number of completed work items in this phase.
    pub completed: usize,

    /// Known number of work items, absent when discovery is ongoing.
    pub total: Option<usize>,

    /// Source path or native module identity associated with the work.
    pub source: Option<String>,
}
