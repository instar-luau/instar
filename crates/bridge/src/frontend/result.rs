use instar_analysis::{Completion, Diagnostic, Location};
use serde::{Deserialize, Serialize};

/// An upstream native warning anchored to a host source revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Warning {
    /// Warning source range.
    pub location: Location<String>,

    /// Upstream warning code.
    pub code: i32,

    /// Upstream warning name.
    pub name: String,

    /// Upstream warning message.
    pub message: String,

    /// Whether native configuration promotes this warning to an error.
    pub fatal: bool,
}

/// An inferred semantic property used by Instar lint rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FactKind {
    /// An unannotated local binding inferred as any.
    ImplicitAnyLocal,

    /// An unannotated function parameter inferred as any.
    ImplicitAnyParameter,
}

/// A detached semantic finding from the native type graph.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Fact {
    /// Binding source range.
    pub location: Location<String>,

    /// Semantic property.
    pub kind: FactKind,

    /// Description of the semantic property.
    pub message: String,
}

/// Native lint output, retaining diagnostics when analysis is incomplete.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LintResult {
    /// Selected entries and their host-resolved dependencies.
    pub modules: Vec<String>,

    /// Upstream warnings and native fatal promotion.
    pub warnings: Vec<Warning>,

    /// Requested inferred semantic properties.
    pub facts: Vec<Fact>,

    /// Syntax, declaration environment and analysis failures.
    pub diagnostics: Vec<Diagnostic<String>>,

    /// Whether all requested warning and semantic checks completed.
    pub completion: Completion,
}
