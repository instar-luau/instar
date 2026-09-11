use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(serde::Serialize)]
pub enum Whitespace {
    #[default]
    Tabs,

    Spaces,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[derive(serde::Serialize)]
pub enum Endings {
    #[default]
    Unix,

    Windows,
}

impl Endings {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Unix => "\n",
            Self::Windows => "\r\n",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Quotes {
    #[default]
    PreferDouble,

    PreferSingle,
    Double,
    Single,
    Preserve,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Zero {
    #[default]
    Add,

    Strip,
    Preserve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Parentheses {
    #[default]
    Always,

    OmitString,
    OmitTable,

    OmitOptional,

    Preserve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Separation {
    #[default]
    Never,

    Definitions,
    Calls,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Semicolons {
    #[default]
    Never,

    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Expansion {
    #[default]
    Needed,

    Always,
    Never,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Separator {
    #[default]
    Comma,

    Semicolon,
}

impl Separator {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Comma => ",",
            Self::Semicolon => ";",
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Tables {
    /// Format table-type layouts. When false, multiline table types retain their source layout and single-line table types stay flat.
    pub enabled: bool,

    /// Remove or preserve existing blank lines between members without inserting new gaps.
    pub blank_lines: Gaps,

    /// Maximum flat table-type width before expansion, also constrained by `column_width`.
    pub width: usize,

    /// Separate table-type members with a comma or semicolon.
    pub separator: Separator,

    /// Preserve field order, sort alphabetically, by key length, or by formatted field width. Length and width orders use alphabetical tie-breaking. Comments prevent sorting.
    pub order: Order,

    /// Place indexers first, last, or among sorted fields. Used only when order changes; sorted indexers use an empty key.
    pub indexer: Indexer,
}

impl Default for Tables {
    fn default() -> Self {
        Self {
            enabled: true,
            blank_lines: Gaps::default(),
            width: 60,
            separator: Separator::default(),
            order: Order::default(),
            indexer: Indexer::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum ConditionalExpansion {
    #[default]
    Never,

    Always,
    Needed,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum ConditionalStyle {
    #[default]
    Block,

    Leading,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Placement {
    #[default]
    SameLine,

    NextLine,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Conditional {
    /// Expand conditional expressions when needed by width, always at the outermost level, or never voluntarily. Column-width wrapping still applies.
    pub expand: ConditionalExpansion,

    /// Flat expression width that triggers expansion when expand is needed or always.
    pub width: usize,

    /// Block places branch values on indented lines; leading places then and else before their values on indented lines.
    pub style: ConditionalStyle,

    /// Same-line starts the expression after its binding or return; next-line moves an expanded sole expression to an indented line.
    pub placement: Placement,

    /// Number of additional indentation levels for expanded conditional branches.
    pub indentation: usize,
}

impl Default for Conditional {
    fn default() -> Self {
        Self {
            expand: ConditionalExpansion::default(),
            width: 60,
            style: ConditionalStyle::default(),
            placement: Placement::default(),
            indentation: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum CallStyle {
    #[default]
    OnePerLine,

    HugLast,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Calls {
    /// Expand argument lists when needed by width, always, or never voluntarily. Mandatory line breaks are retained.
    pub expand: Expansion,

    /// One-per-line separates expanded arguments; hug-last keeps a final table, function, or multiline string attached to preceding arguments unless expansion is always. A sole string, table, or function remains attached in either style.
    pub style: CallStyle,

    /// Number of additional indentation levels for expanded arguments.
    pub indentation: usize,

    /// Always use parentheses, omit them for a sole string or table argument, omit either optional form, or preserve the input choice.
    pub parentheses: Parentheses,

    /// Layout of repeated dot and colon calls.
    pub chains: Chains,
}

impl Default for Calls {
    fn default() -> Self {
        Self {
            expand: Expansion::default(),
            style: CallStyle::default(),
            indentation: 1,
            parentheses: Parentheses::default(),
            chains: Chains::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Parameters {
    /// Expand declaration parameters when needed by width, always, or never voluntarily. Mandatory line breaks are retained.
    pub expand: Expansion,

    /// Number of additional indentation levels for expanded declaration parameters.
    pub indentation: usize,
}

impl Default for Parameters {
    fn default() -> Self {
        Self {
            expand: Expansion::default(),
            indentation: 1,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Indentation {
    /// Indent with tabs or spaces.
    pub style: Whitespace,

    /// Columns per indentation level, including the display width of a tab. Must be positive when formatting is enabled.
    pub width: usize,
}

impl Default for Indentation {
    fn default() -> Self {
        Self {
            style: Whitespace::default(),
            width: 4,
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Spacing {
    /// Add interior spaces to nonempty parentheses.
    pub parentheses: bool,

    /// Add interior spaces to nonempty brackets.
    pub brackets: bool,

    /// Add interior spaces to nonempty braces.
    pub braces: bool,

    /// Add a space before function parentheses: never, definitions, calls, or always.
    pub function_names: Separation,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            parentheses: false,
            brackets: false,
            braces: true,
            function_names: Separation::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, JsonSchema, serde::Serialize)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent formatting controls"
)]
pub struct Options {
    /// Format source files and run formatting grafts.
    pub enabled: bool,

    /// Include matching source paths. Patterns are relative to the declaring configuration and override format exclusions. Explicit file inputs bypass selection. An empty list clears inherited patterns.
    pub include: Vec<String>,

    /// Exclude matching source paths unless included at format level. Patterns are relative to the declaring configuration. Explicit file inputs bypass selection. An empty list clears inherited patterns.
    pub exclude: Vec<String>,

    /// Target line width in columns. Must be positive when formatting is enabled. Unbreakable syntax may exceed it.
    pub column_width: usize,

    /// Indentation characters and columns per level.
    pub indentation: Indentation,

    /// Unix emits line feeds; windows emits carriage return plus line feed.
    pub line_endings: Endings,

    /// End nonempty formatted output with a newline.
    pub final_newline: bool,

    /// Prefer-double and prefer-single choose the delimiter with fewer unescaped occurrences in the source, using the preference for ties. Double and single enforce a delimiter; preserve retains it. Long strings retain their delimiters.
    pub quotes: Quotes,

    /// Add, strip, or preserve the zero before the decimal point in fractional numeric literals.
    pub leading_zero: Zero,

    /// Never emits statement semicolons only where required to preserve parsing; always emits them after eligible statements.
    pub semicolons: Semicolons,

    /// Interior delimiter spaces and spacing before function parentheses.
    pub spacing: Spacing,

    /// Expand table expressions whose source contains a trailing comma.
    pub expand_on_trailing_comma: bool,

    /// Emit a trailing separator in expanded tables and table types, using the configured table-type separator where applicable.
    pub trailing_separator: bool,

    /// Statement block layout and existing blank lines.
    pub blocks: Blocks,

    /// Argument parentheses, list layout, and repeated calls.
    pub calls: Calls,

    /// Function parameter layout and declaration binding rewrites.
    pub functions: Functions,

    /// Layout of if-then-else expressions, not conditional statements.
    pub conditionals: Conditional,

    /// Field ordering in table expressions.
    pub tables: Sorting,

    /// Table-type fields and union and intersection layouts.
    pub types: Types,

    /// Require binding ordering, grouping, conversion, and unused binding handling.
    pub imports: Imports,

    /// Scope-aware conversion of local bindings to constants.
    pub bindings: Constants,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: true,
            include: Vec::new(),
            exclude: Vec::new(),
            column_width: 120,
            indentation: Indentation::default(),
            line_endings: Endings::default(),
            final_newline: true,
            quotes: Quotes::default(),
            leading_zero: Zero::default(),
            semicolons: Semicolons::default(),
            spacing: Spacing::default(),
            expand_on_trailing_comma: true,
            trailing_separator: true,
            blocks: Blocks::default(),
            calls: Calls::default(),
            functions: Functions::default(),
            conditionals: Conditional::default(),
            tables: Sorting::default(),
            types: Types::default(),
            imports: Imports::default(),
            bindings: Constants::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Collapse {
    #[default]
    Never,

    Functions,
    Conditionals,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Gaps {
    #[default]
    Remove,

    Preserve,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Blocks {
    /// Collapse eligible single-statement blocks: never, functions, conditionals, or always. Comments can prevent collapsing.
    pub collapse: Collapse,

    /// Remove or preserve one existing blank line at each block boundary. Interior statement gaps are retained independently.
    pub blank_lines: Gaps,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Binding {
    #[default]
    Preserve,

    Const,
    Local,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Unused {
    #[default]
    Ignore,

    Underscore,
    Remove,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Declaration {
    #[default]
    Preserve,

    Local,
    Const,
    Global,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Grouping {
    #[default]
    Flat,

    ByKind,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Imports {
    /// Sort eligible adjacent require bindings by module path.
    pub sort: bool,

    /// Flat sorts imports together; by-kind groups alias paths, other paths, then relative paths, inserting blank lines between categories. Only applies when sorting.
    pub grouping: Grouping,

    /// Preserve require binding declarations or convert eligible bindings to const or local.
    pub binding: Binding,

    /// Ignore unused require bindings, prefix their names with an underscore, or remove eligible declarations.
    pub unused: Unused,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Constants {
    /// Convert eligible unreassigned local bindings to const using lexical scope analysis.
    pub prefer_constant: bool,

    /// Keep bindings to mutated tables local when preferring constant bindings.
    pub preserve_mutated_tables: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Order {
    #[default]
    Preserve,

    KeyLengthAscending,
    KeyLengthDescending,
    Alphabetical,
    FieldWidthAscending,
    FieldWidthDescending,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Indexer {
    #[default]
    First,

    Last,
    Sorted,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Sorting {
    /// Preserve field order, sort alphabetically, by key length, or by formatted field width. Length and width orders use alphabetical tie-breaking. Comment-bearing tables retain their order.
    pub order: Order,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Chain {
    #[default]
    Preserve,

    Method,
    Full,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Chains {
    /// Preserve ordinary call layout; method keeps the first call with the receiver; full allows a break before every call. Both method and full handle dot and colon calls.
    pub style: Chain,

    /// Force chain expansion at this many calls. Zero disables count-based expansion; width-based wrapping remains available.
    pub minimum_calls: usize,
}

impl Default for Chains {
    fn default() -> Self {
        Self {
            style: Chain::default(),
            minimum_calls: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TypeExpansion {
    #[default]
    Needed,

    Always,
    Never,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Operators {
    /// Expand unions and intersections when needed, always, or never voluntarily. Nested types and overload signatures remain grouped where possible.
    pub expand: TypeExpansion,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Types {
    /// Layout and ordering of fields in table types.
    pub tables: Tables,

    /// Layout of union and intersection members.
    pub operators: Operators,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Functions {
    /// Layout of function declaration parameters.
    pub parameters: Parameters,

    /// Preserve function declarations or convert eligible declarations to local, const, or global forms.
    pub binding: Declaration,
}
