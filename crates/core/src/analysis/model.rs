use std::ops::Range;

/// Owned source facts whose ranges are UTF-8 byte offsets in the original document.
#[derive(Clone, Debug)]
pub struct Analysis {
    /// Hierarchical document outline.
    pub symbols: Vec<Symbol>,

    /// Ordered, nonoverlapping lexical and binding classifications.
    pub tokens: Vec<Token>,

    /// Collapsible syntax and comment regions.
    pub folds: Vec<Fold>,

    /// Import arguments awaiting workspace resolution.
    pub imports: Vec<Import>,

    /// Explicit documentation addresses.
    pub links: Vec<Link>,

    /// Recognized Roblox color expressions.
    pub colors: Vec<Color>,

    /// Lexically resolved value and type bindings.
    pub bindings: Vec<Binding>,

    pub(super) source: String,
    pub(super) selections: Vec<Range<usize>>,
    pub(super) identities: Vec<Identity>,
    pub(super) scopes: Vec<usize>,
    pub(super) complete: bool,
}

/// A named declaration and its nested declarations.
#[derive(Clone, Debug)]
pub struct Symbol {
    /// Source spelling of the declared name.
    pub name: String,

    /// Declaration category.
    pub kind: SymbolKind,

    /// Entire declaration.
    pub range: Range<usize>,

    /// Declared name.
    pub selection: Range<usize>,

    /// Declarations nested within this declaration.
    pub children: Vec<Self>,
}

/// Document outline categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SymbolKind {
    /// Callable declaration.
    Function,

    /// Receiver-qualified callable declaration.
    Method,

    /// Mutable value declaration.
    Variable,

    /// Immutable value declaration.
    Constant,

    /// Type alias declaration.
    Type,

    /// Class declaration.
    Class,

    /// Named table or class member.
    Property,
}

/// A classified source token.
#[derive(Clone, Debug)]
pub struct Token {
    /// Token byte range.
    pub range: Range<usize>,

    /// Semantic lexical category.
    pub kind: TokenKind,

    /// Whether this occurrence declares a binding.
    pub declaration: bool,

    /// Whether the referenced binding is immutable.
    pub readonly: bool,

    /// Binding access performed by this occurrence.
    pub access: Access,
}

/// Binding access performed by a token.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Access {
    /// No binding access.
    None,

    /// Reads a binding.
    Read,

    /// Writes or declares a binding.
    Write,

    /// Reads and writes a binding.
    ReadWrite,
}

impl Access {
    /// Whether this access reads a binding.
    #[must_use]
    pub fn read(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    /// Whether this access writes a binding.
    #[must_use]
    pub fn write(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }

    pub(super) fn new(read: bool, write: bool) -> Self {
        match (read, write) {
            (false, false) => Self::None,
            (true, false) => Self::Read,
            (false, true) => Self::Write,
            (true, true) => Self::ReadWrite,
        }
    }
}

/// Protocol-independent semantic token categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenKind {
    /// Module namespace.
    Namespace,

    /// Type reference or alias.
    Type,

    /// Generic type parameter.
    TypeParameter,

    /// Function parameter.
    Parameter,

    /// Value binding or global.
    Variable,

    /// Table or class property.
    Property,

    /// Callable value.
    Function,

    /// Receiver-qualified callable member.
    Method,

    /// Reserved or contextual keyword.
    Keyword,

    /// Source comment or directive.
    Comment,

    /// String literal or interpolation segment.
    String,

    /// Numeric literal.
    Number,

    /// Operator or punctuation.
    Operator,

    /// Function attribute.
    Decorator,
}

/// A collapsible byte range.
#[derive(Clone, Debug)]
pub struct Fold {
    /// Collapsible source region.
    pub range: Range<usize>,

    /// Region category.
    pub kind: FoldKind,
}

/// Folding region categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FoldKind {
    /// Structural syntax region.
    Region,

    /// Comment region.
    Comment,

    /// Consecutive import declarations.
    Imports,
}

/// A require argument eligible for workspace resolution.
#[derive(Clone, Debug)]
pub struct Import {
    /// Full argument expression, including string delimiters.
    pub range: Range<usize>,

    /// Decoded path or original instance expression.
    pub expression: ImportExpression,
}

/// Supported import argument forms.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportExpression {
    /// A decoded string require path.
    Path(String),

    /// A script- or game-rooted instance expression.
    Instance(String),
}

/// An explicit documentation URL in source.
#[derive(Clone, Debug)]
pub struct Link {
    /// URL spelling in the source.
    pub range: Range<usize>,

    /// Validated absolute HTTP or HTTPS URL.
    pub target: String,
}

/// A statically recognized Roblox Color3 constructor.
#[derive(Clone, Debug)]
pub struct Color {
    /// Complete constructor expression.
    pub range: Range<usize>,

    /// Red, green, blue, and alpha in the inclusive zero-to-one interval.
    pub components: [f64; 4],
}

/// A lexical binding and every source occurrence resolved to it.
#[derive(Clone, Debug)]
pub struct Binding {
    /// Declared name.
    pub name: String,

    /// Binding namespace and category.
    pub kind: BindingKind,

    /// Declaration name range.
    pub declaration: Range<usize>,

    /// Declaration and all resolved uses, in source order.
    pub occurrences: Vec<Occurrence>,

    /// Whether the declaration has an explicit, renameable source name.
    pub renameable: bool,

    /// Whether this declaration is an exported type alias or type function.
    pub exported: bool,
}

/// Lexical binding categories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingKind {
    /// Ordinary local value.
    Local,

    /// Function parameter.
    Parameter,

    /// Loop iteration variable.
    Loop,

    /// Local type alias.
    TypeAlias,

    /// Generic type or pack parameter.
    TypeParameter,
}

/// A resolved occurrence of a lexical binding.
#[derive(Clone, Debug)]
pub struct Occurrence {
    /// Identifier byte range.
    pub range: Range<usize>,

    /// Whether this occurrence reads the binding.
    pub read: bool,

    /// Whether this occurrence writes or declares the binding.
    pub write: bool,
}

/// A validated lexical rename target.
#[derive(Clone, Debug)]
pub struct RenameTarget {
    /// Selected identifier range.
    pub range: Range<usize>,

    /// Existing name for a rename prompt.
    pub name: String,
}

/// A protocol-independent source replacement.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Edit {
    /// Original source byte range.
    pub range: Range<usize>,

    /// Replacement source text.
    pub replacement: String,
}

/// A rejected lexical rename.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RenameError {
    /// The position does not identify a renameable local binding.
    #[error("no renameable lexical binding at this position")]
    Target,

    /// The replacement is not a valid Luau identifier.
    #[error("the replacement is not a valid Luau identifier")]
    Name,

    /// The document is incomplete and lexical safety cannot be established.
    #[error("rename requires a syntactically complete document")]
    Incomplete,

    /// Renaming would conflict with or capture another binding.
    #[error("the replacement conflicts with or captures another binding")]
    Conflict,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Identity {
    Value(usize),
    Type(usize),
    Global(String),
    External(String),
}
