/// Type analysis settings and source selection.
pub mod analyze;

/// Build settings and transformation policies.
pub mod build;

/// Formatter settings and layout policies.
pub mod format;

/// Lint settings and rule policies.
pub mod lint;

/// Roblox environment settings and API permissions.
pub mod roblox;

mod schema;

pub use analyze::AnalyzeConfig;
pub use roblox::{RobloxConfig, RobloxLevel};

use schemars::JsonSchema;
use serde::{Deserialize, de::Error};
use std::collections::BTreeMap;

pub(crate) fn overlay(
    under: &mut serde_json::Map<String, serde_json::Value>,
    over: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in over {
        match (under.get_mut(&key), value) {
            (Some(serde_json::Value::Object(under)), serde_json::Value::Object(over)) => {
                overlay(under, over);
            }

            (_, value) => {
                under.insert(key, value);
            }
        }
    }
}

/// Settings read from an `instar.toml` project configuration.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstarConfig {
    /// Source builds, bundle output, runtime targets, compilation, and named build profiles.
    #[schemars(default)]
    pub build: Option<build::Settings>,

    /// Lint rules, groups, globals, selection, and options. Settings inherit from ancestor configurations.
    #[schemars(default)]
    pub lint: Option<lint::Settings>,

    /// Type checking, external definitions and documentation, aliases, Roblox integration, and source selection.
    #[schemars(default)]
    pub analyze: Option<analyze::AnalyzeConfig>,

    /// Formatter settings. Ancestor settings merge field by field; absent settings use their documented defaults.
    #[schemars(default)]
    pub format: Option<format::Options>,

    /// Graft projects loaded from local paths or installed GitHub releases. Entries inherit by name and run in name order.
    #[schemars(default)]
    pub grafts: Option<BTreeMap<String, crate::graft::Dependency>>,

    /// Source selection patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub include: Option<Vec<String>>,

    /// Source exclusion patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub exclude: Option<Vec<String>>,
}

impl InstarConfig {
    /// # Errors
    /// Returns invalid TOML, unknown fields or field-type errors with TOML spans.
    pub fn parse(text: &str) -> Result<Self, toml_edit::de::Error> {
        let configuration: Self = toml_edit::de::from_str(text)?;

        for (name, dependency) in configuration.grafts.iter().flatten() {
            dependency
                .validate(name)
                .map_err(toml_edit::de::Error::custom)?;
        }

        Ok(configuration)
    }
}
