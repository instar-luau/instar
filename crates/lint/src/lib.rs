//! Luau source linting.

mod bindings;
pub mod configuration;
pub mod literal;
mod rules;

pub use configuration::identity::Rule;
pub use configuration::{Configuration, Level};
pub use instar_check::{Cancellation, Completion, Location, Options, Reason, Related};

/// Lint diagnostic identity, independent of presentation text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A configured Instar rule.
    Rule(Rule),

    /// An upstream warning retaining its native identity.
    Native {
        /// Upstream warning code.
        code: i32,
        /// Upstream warning name.
        name: String,
    },

    /// Source could not be parsed.
    Syntax,

    /// A host require request could not be resolved.
    Resolution,

    /// Analysis or declaration loading did not complete.
    Analysis,

    /// A requested capability is unavailable.
    Unsupported,
}

/// A revision-anchored lint finding or analysis error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic<Module> {
    /// Exact host source identity and byte range.
    pub location: Location<Module>,

    /// Structured rule or analysis identity.
    pub kind: Kind,

    /// Effective Instar severity or native warning policy.
    pub level: Level,

    /// Explanation of the finding.
    pub message: String,

    /// Additional source context.
    pub related: Vec<Related<Module>>,
}

/// Lint output that explicitly distinguishes partial analysis from success.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Result<Module> {
    /// Selected contextual modules, once per identity.
    pub modules: Vec<Module>,

    /// Findings at their actual immutable source revisions.
    pub diagnostics: Vec<Diagnostic<Module>>,

    /// Whether all requested analysis completed.
    pub completion: Completion,
}

/// A typed require site supplied by the host's shared resolver.
#[derive(Clone, Debug)]
pub struct Require {
    /// Entire call byte range.
    pub call: [usize; 2],

    /// Argument byte range.
    pub argument: [usize; 2],

    /// Whether the argument is statically known.
    pub constant: bool,

    /// Decoded constant string path, if the request is a string.
    pub path: Option<String>,
}

/// Syntax analysis inputs that do not require native type inference.
pub struct Source<'source> {
    /// Immutable source text.
    pub text: &'source str,

    /// Source revision anchoring findings.
    pub revision: u64,

    /// Effective Instar rule configuration.
    pub configuration: &'source Configuration,

    /// Additional globals supplied by native configuration.
    pub globals: &'source [String],

    /// Whether Roblox syntax rules are applicable.
    pub roblox: bool,

    /// Require sites from the shared typed extraction and resolution graph.
    pub requires: &'source [Require],

    /// Native inferred-any facts; omission reports unavailable enabled type rules.
    pub inferred: Option<&'source [Inference]>,
}

/// One unannotated binding whose shared native inferred type is any.
#[derive(Clone, Debug)]
pub struct Inference {
    /// Binding name byte range.
    pub range: [usize; 2],

    /// Whether this binding is a function parameter.
    pub parameter: bool,
}

/// Evaluates syntax and lexical binding rules without native analysis or fixes.
///
/// # Errors
/// Returns invalid rule configuration; parse failures are structured diagnostics.
pub fn lint<Module: Clone>(
    module: Module,
    source: &Source<'_>,
    options: &Options,
) -> std::io::Result<Result<Module>> {
    rules::lint(module, source, options)
}
