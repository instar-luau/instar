use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Characters used for each indentation level.
pub enum Whitespace {
    /// Indent with tab characters.
    #[default]
    Tabs,

    /// Indent with space characters.
    Spaces,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Line-ending sequence emitted by the formatter.
pub enum Endings {
    /// Emit a line feed.
    #[default]
    Unix,

    /// Emit a carriage return followed by a line feed.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Delimiter selection for short string literals.
pub enum Quotes {
    /// Minimize escapes, preferring double quotes on ties.
    #[default]
    PreferDouble,

    /// Minimize escapes, preferring single quotes on ties.
    PreferSingle,

    /// Use double quotes.
    Double,

    /// Use single quotes.
    Single,

    /// Retain the original delimiter.
    Preserve,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Leading-zero treatment for fractional numeric literals.
pub enum Zero {
    /// Insert a zero before a leading decimal point.
    #[default]
    Add,

    /// Remove a zero before a leading decimal point.
    Strip,

    /// Retain the source spelling.
    Preserve,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Parentheses policy for function arguments.
pub enum Parentheses {
    /// Parenthesize every argument list.
    #[default]
    Always,

    /// Omit parentheses around a sole string argument.
    OmitString,

    /// Omit parentheses around a sole table argument.
    OmitTable,

    /// Omit parentheses around either optional argument form.
    OmitOptional,

    /// Retain the source choice where syntax permits.
    Preserve,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Spaces between function names and opening parentheses.
pub enum Separation {
    /// Keep names adjacent to parentheses.
    #[default]
    Never,

    /// Insert a space in function declarations only.
    Definitions,

    /// Insert a space in function calls only.
    Calls,

    /// Insert a space in declarations and calls.
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Statement terminator policy.
pub enum Semicolons {
    /// Emit semicolons only where required to preserve parsing.
    #[default]
    Never,

    /// Terminate every eligible statement with a semicolon.
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Voluntary expansion of lists across lines.
pub enum Expansion {
    /// Expand when the list exceeds the available width.
    #[default]
    Needed,

    /// Expand eligible lists regardless of width.
    Always,

    /// Keep lists flat unless a mandatory break requires expansion.
    Never,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Punctuation separating table-type members.
pub enum Separator {
    /// Separate members with commas.
    #[default]
    Comma,

    /// Separate members with semicolons.
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Layout and ordering of table-type members.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Expansion policy for conditional expressions.
pub enum ConditionalExpansion {
    /// Expand only when required by the column width.
    #[default]
    Never,

    /// Expand outer conditional expressions regardless of width.
    Always,

    /// Expand expressions exceeding the configured conditional width.
    Needed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Placement of keywords in expanded conditional expressions.
pub enum ConditionalStyle {
    /// Put branch values on indented lines after their keywords.
    #[default]
    Block,

    /// Begin indented branch lines with then or else.
    Leading,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Placement of an expanded expression after its binding or return.
pub enum Placement {
    /// Start on the same line as the preceding syntax.
    #[default]
    SameLine,

    /// Start a sole expanded expression on an indented line.
    NextLine,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Layout of if-then-else expressions.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Layout of expanded call arguments.
pub enum CallStyle {
    /// Give expanded arguments separate lines.
    #[default]
    OnePerLine,

    /// Keep a final table, function, or multiline string attached when possible.
    HugLast,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Argument-list and chained-call formatting.
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Layout of function declaration parameter lists.
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Indentation characters and display width.
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Interior delimiter and function-name spacing.
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

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Independent formatting controls"
)]
/// Complete formatter settings with inherited project defaults.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Eligible single-statement blocks to collapse onto one line.
pub enum Collapse {
    /// Keep block bodies on separate lines.
    #[default]
    Never,

    /// Collapse eligible function bodies.
    Functions,

    /// Collapse eligible conditional branches.
    Conditionals,

    /// Collapse both eligible function bodies and conditional branches.
    Always,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Treatment of existing blank lines at layout boundaries.
pub enum Gaps {
    /// Remove boundary blank lines.
    #[default]
    Remove,

    /// Retain one existing boundary blank line.
    Preserve,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Statement-block collapsing and boundary spacing.
pub struct Blocks {
    /// Collapse eligible single-statement blocks: never, functions, conditionals, or always. Comments can prevent collapsing.
    pub collapse: Collapse,

    /// Remove or preserve one existing blank line at each block boundary. Interior statement gaps are retained independently.
    pub blank_lines: Gaps,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Declaration form for eligible require bindings.
pub enum Binding {
    /// Keep the source declaration form.
    #[default]
    Preserve,

    /// Convert eligible bindings to const.
    Const,

    /// Convert eligible bindings to local.
    Local,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Treatment of unused require bindings.
pub enum Unused {
    /// Leave unused bindings unchanged.
    #[default]
    Ignore,

    /// Prefix unused binding names with an underscore.
    Underscore,

    /// Remove eligible unused declarations.
    Remove,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Declaration form for eligible function definitions.
pub enum Declaration {
    /// Keep the source declaration form.
    #[default]
    Preserve,

    /// Use a local function declaration.
    Local,

    /// Use a constant function declaration.
    Const,

    /// Use a global function declaration.
    Global,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Grouping of sorted require bindings.
pub enum Grouping {
    /// Sort all adjacent imports together.
    #[default]
    Flat,

    /// Separate alias, other, and relative paths into ordered groups.
    ByKind,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Ordering, declaration conversion, and removal of require bindings.
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

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Scope-aware conversion of local bindings to constants.
pub struct Constants {
    /// Convert eligible unreassigned local bindings to const using lexical scope analysis.
    pub prefer_constant: bool,

    /// Keep bindings to mutated tables local when preferring constant bindings.
    pub preserve_mutated_tables: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Ordering of eligible table fields.
pub enum Order {
    /// Retain the source order.
    #[default]
    Preserve,

    /// Sort shorter keys first, breaking ties alphabetically.
    KeyLengthAscending,

    /// Sort longer keys first, breaking ties alphabetically.
    KeyLengthDescending,

    /// Sort keys alphabetically.
    Alphabetical,

    /// Sort narrower formatted fields first, breaking ties alphabetically.
    FieldWidthAscending,

    /// Sort wider formatted fields first, breaking ties alphabetically.
    FieldWidthDescending,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Placement of table-type indexers when fields are sorted.
pub enum Indexer {
    /// Put indexers before named fields.
    #[default]
    First,

    /// Put indexers after named fields.
    Last,

    /// Sort indexers alongside fields using an empty key.
    Sorted,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Ordering of fields in table expressions.
pub struct Sorting {
    /// Preserve field order, sort alphabetically, by key length, or by formatted field width. Length and width orders use alphabetical tie-breaking. Comment-bearing tables retain their order.
    pub order: Order,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Line-break policy for repeated dot and colon calls.
pub enum Chain {
    /// Retain ordinary call layout.
    #[default]
    Preserve,

    /// Keep the first call with its receiver.
    Method,

    /// Allow a line break before every call.
    Full,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Style and count threshold for expanding call chains.
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
/// Expansion policy for union and intersection members.
pub enum TypeExpansion {
    /// Expand when members exceed the available width.
    #[default]
    Needed,

    /// Expand eligible members regardless of width.
    Always,

    /// Avoid voluntary expansion while retaining required breaks.
    Never,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Layout of union and intersection operators.
pub struct Operators {
    /// Expand unions and intersections when needed, always, or never voluntarily. Nested types and overload signatures remain grouped where possible.
    pub expand: TypeExpansion,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Table-type and compound-type layout settings.
pub struct Types {
    /// Layout and ordering of fields in table types.
    pub tables: Tables,

    /// Layout of union and intersection members.
    pub operators: Operators,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Function parameter layout and declaration conversion.
pub struct Functions {
    /// Layout of function declaration parameters.
    pub parameters: Parameters,

    /// Preserve function declarations or convert eligible declarations to local, const, or global forms.
    pub binding: Declaration,
}
