//! Type-checking configuration.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema;

/// Type-checking settings and file selection.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Replacement inclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub include: Option<Vec<String>>,

    /// Replacement exclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub exclude: Option<Vec<String>>,

    /// Checking mode for sources without an overriding directive.
    pub mode: Mode,
}

/// Luau type-checking mode.
#[derive(Copy, Clone, Debug, Default, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Strict type inference.
    Strict,

    /// Nonstrict type inference.
    #[default]
    Nonstrict,

    /// Disable ordinary type diagnostics.
    Nocheck,
}
