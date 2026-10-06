use instar_analysis::Completion;

use crate::Diagnostic;

/// Lint output that explicitly distinguishes partial analysis from success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Result<Module> {
    /// Modules in the selected reachable graph, once per exact identity.
    pub modules: Vec<Module>,

    /// Findings at their actual immutable source revisions.
    pub diagnostics: Vec<Diagnostic<Module>>,

    /// Whether all requested analysis completed.
    pub completion: Completion,
}
