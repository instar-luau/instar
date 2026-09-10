use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Collapse {
    #[default]
    Never,

    FunctionOnly,
    ConditionalOnly,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Gaps {
    #[default]
    Never,

    Preserve,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Binding {
    #[default]
    Preserve,

    Const,
    Local,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Imports {
    #[default]
    Ignore,

    Underscore,
    Remove,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Functions {
    #[default]
    Preserve,

    Local,
    Const,
    Global,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Grouping {
    #[default]
    Flat,

    ByKind,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Requires {
    pub enabled: bool,
    pub grouping: Grouping,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Constants {
    pub enabled: bool,
    pub mutated_tables_stay_local: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
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
#[derive(serde::Serialize)]
pub enum Indexer {
    #[default]
    First,

    Last,
    Sorted,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
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
#[derive(serde::Serialize)]
pub struct Sorting {
    pub order: Order,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
#[derive(serde::Serialize)]
pub enum Chain {
    #[default]
    Preserve,

    Method,
    Full,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
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
#[derive(serde::Serialize)]
pub enum TypeExpansion {
    #[default]
    Auto,

    Always,
    Never,
}

#[derive(Debug, Clone, Default, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[derive(serde::Serialize)]
pub struct Operators {
    pub expand: TypeExpansion,
}
