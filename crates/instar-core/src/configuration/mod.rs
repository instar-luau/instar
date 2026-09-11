pub mod format;
mod schema;

use schemars::JsonSchema;
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstarConfig {
    /// Formatter settings. Ancestor settings merge field by field; absent settings use their documented defaults.
    #[schemars(default)]
    pub format: Option<format::Options>,

    /// Graft names mapped to manifest paths relative to this configuration. Entries inherit by name and run in name order.
    #[schemars(default)]
    pub grafts: Option<BTreeMap<String, PathBuf>>,

    /// Source selection patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub include: Option<Vec<String>>,

    /// Source exclusion patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub exclude: Option<Vec<String>>,

    /// External Luau definition files, resolved relative to this configuration.
    #[schemars(default)]
    pub definitions: Option<Vec<PathBuf>>,

    /// Require aliases mapped to paths relative to this configuration. Entries inherit by name.
    #[schemars(default)]
    pub aliases: Option<BTreeMap<String, PathBuf>>,

    /// Roblox project inputs. Project mapping integration is not yet implemented.
    #[schemars(default)]
    pub roblox: Option<RobloxConfig>,
}

#[derive(Debug, Deserialize, JsonSchema, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RobloxConfig {
    /// Rojo project path relative to this configuration. Project mapping integration is not yet implemented.
    #[schemars(default)]
    pub project: Option<PathBuf>,

    /// Existing instance-to-source mapping path relative to this configuration. Sourcemap integration is not yet implemented; Instar does not generate this file.
    #[schemars(default)]
    pub sourcemap: Option<PathBuf>,
}

impl InstarConfig {
    /// # Errors
    /// Returns invalid TOML, unknown fields or field-type errors with TOML spans.
    pub fn parse(text: &str) -> Result<Self, toml_edit::de::Error> {
        toml_edit::de::from_str(text)
    }
}
