//! Declaration and Roblox environment configuration.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema;

/// Environment inputs, with omission preserving automatic Roblox selection.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Explicit Roblox selection, or automatic selection when omitted.
    pub roblox: Option<bool>,

    /// Roblox capability security profile.
    pub security: Security,

    /// Additional declared global names.
    #[schemars(extend("items" = schema::identifier()))]
    pub globals: Vec<String>,

    /// Custom capability input file.
    #[schemars(extend("anyOf" = [schema::path(), { "type": "null" }]))]
    pub capabilities: Option<PathBuf>,

    /// Ordered declaration and documentation groups.
    #[schemars(extend("uniqueItems" = true))]
    pub groups: Vec<Group>,
}

/// An ordered declaration and documentation namespace.
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Group {
    /// Custom namespace beginning with `@`.
    #[schemars(regex(pattern = "^@[A-Za-z0-9_-]+$"), extend("not" = { "enum": ["@luau", "@roblox"] }))]
    pub namespace: String,

    /// Ordered declaration source files.
    #[serde(default)]
    #[schemars(extend("items" = schema::path()))]
    pub definitions: Vec<PathBuf>,

    /// Ordered documentation files.
    #[serde(default)]
    #[schemars(extend("items" = schema::path()))]
    pub documentation: Vec<PathBuf>,
}

/// Roblox capability security profile.
#[derive(
    Copy, Clone, Debug, Default, Eq, Ord, PartialEq, PartialOrd, Deserialize, JsonSchema, Serialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Standard script capabilities.
    #[default]
    None,

    /// Local-script capabilities.
    Local,

    /// Plugin capabilities.
    Plugin,

    /// Roblox-internal capabilities.
    Roblox,
}
