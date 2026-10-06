use crate::Configuration;

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
