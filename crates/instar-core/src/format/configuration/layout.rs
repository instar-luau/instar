use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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
    pub tables: super::Tables,

    /// Layout of union and intersection members.
    pub operators: Operators,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Functions {
    /// Layout of function declaration parameters.
    pub parameters: super::Parameters,

    /// Preserve function declarations or convert eligible declarations to local, const, or global forms.
    pub binding: Declaration,
}
