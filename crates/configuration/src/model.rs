use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{checking, environment, formatting, linting, resolution, schema};

/// Project configuration; generate its JSON Schema with `schemars::schema_for!`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Shared inclusion patterns, relative to their defining configuration file.
    #[schemars(extend("items" = schema::pattern()))]
    pub include: Option<Vec<String>>,

    /// Shared exclusion patterns, relative to their defining configuration file.
    #[schemars(extend("items" = schema::pattern()))]
    pub exclude: Option<Vec<String>>,

    /// Luau flag overrides; the engine owns registration and application.
    #[schemars(extend("propertyNames" = schema::identifier()))]
    pub flags: BTreeMap<String, Flag>,

    /// Type checking and its file selection.
    pub check: checking::Configuration,

    /// Workspace index file selection.
    pub index: Selection,

    /// Module aliases and sourcemaps.
    pub resolve: resolution::Configuration,

    /// Declarations, globals, and Roblox environment selection.
    pub environment: environment::Configuration,

    /// Formatting and its file selection.
    pub format: formatting::Configuration,

    /// Linting and its file selection.
    pub lint: linting::Configuration,
}

/// File selection with omission retaining the shared selection policy.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Selection {
    /// Replacement inclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub include: Option<Vec<String>>,

    /// Replacement exclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub exclude: Option<Vec<String>>,
}

/// A boolean or signed 32-bit Luau flag value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum Flag {
    /// Boolean flag override.
    Boolean(bool),

    /// Integer flag override.
    Integer(#[schemars(range(min = i32::MIN, max = i32::MAX))] i32),
}
