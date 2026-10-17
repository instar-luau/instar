use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Patterns, check, environment, format, lint, resolution, schema};

/// Project configuration; generate its JSON Schema with `schemars::schema_for!`.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Shared inclusion patterns, relative to their defining configuration file.
    pub include: Option<Patterns>,

    /// Shared exclusion patterns, relative to their defining configuration file.
    pub exclude: Option<Patterns>,

    /// Luau flag overrides; the engine owns registration and application.
    #[schemars(extend("propertyNames" = schema::identifier()))]
    pub flags: BTreeMap<String, Flag>,

    /// Type checking and its file selection.
    pub check: check::Configuration,

    /// Workspace index file selection.
    pub index: Selection,

    /// Module aliases and sourcemaps.
    pub resolve: resolution::Configuration,

    /// Declarations, globals, and Roblox environment selection.
    pub environment: environment::Configuration,

    /// Formatting and its file selection.
    pub format: format::Configuration,

    /// Linting and its file selection.
    pub lint: lint::Configuration,
}

/// File selection with omission retaining the shared selection policy.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Selection {
    /// Replacement inclusion patterns.
    pub include: Option<Patterns>,

    /// Replacement exclusion patterns.
    pub exclude: Option<Patterns>,
}

/// A boolean or signed 32-bit Luau flag value.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(untagged)]
pub enum Flag {
    /// Boolean flag override.
    Boolean(bool),

    /// Integer flag override.
    Integer(#[schemars(range(min = i32::MIN, max = i32::MAX))] i32),
}
