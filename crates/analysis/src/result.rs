use serde::{Deserialize, Serialize};

use crate::{Completion, Diagnostic};

/// Native analysis output with host-owned source identities.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Result<Module> {
    /// Modules in the selected reachable graph, once per exact identity.
    pub modules: Vec<Module>,

    /// Errors reported once at their actual source revisions.
    pub diagnostics: Vec<Diagnostic<Module>>,

    /// Explicit completeness of this analysis.
    pub completion: Completion,
}
