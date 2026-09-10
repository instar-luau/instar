use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Collapse {
    #[default]
    Never,

    FunctionOnly,
    ConditionalOnly,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Gaps {
    #[default]
    Never,

    Preserve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Binding {
    #[default]
    Preserve,

    Const,
    Local,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Imports {
    #[default]
    Ignore,

    Underscore,
    Remove,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Functions {
    #[default]
    Preserve,

    Local,
    Const,
    Global,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Grouping {
    #[default]
    Flat,

    ByKind,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Requires {
    pub enabled: bool,
    pub grouping: Grouping,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Constants {
    pub enabled: bool,
    pub mutated_tables_stay_local: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Order {
    #[default]
    None,

    Ascending,
    Descending,
    Alphabetical,
    SizeAscending,
    SizeDescending,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Indexer {
    #[default]
    First,

    Last,
    Sorted,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Properties {
    pub order: Order,
    pub indexer: Indexer,
}

impl Properties {
    pub(in crate::format) fn position(&self) -> Indexer {
        self.indexer
    }
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Sorting {
    pub order: Order,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum Chain {
    #[default]
    Preserve,

    Method,
    Full,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Chains {
    pub style: Chain,
    pub min_calls: usize,
}

impl Default for Chains {
    fn default() -> Self {
        Self {
            style: Chain::default(),
            min_calls: 3,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum TypeExpansion {
    #[default]
    Auto,

    Always,
    Never,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Operators {
    pub expand: TypeExpansion,
}
