pub mod format;
mod schema;

use schemars::JsonSchema;
use serde::Deserialize;
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstarConfig {
    #[schemars(
        default,
        description = "Checking mode. Inherits from ancestor configurations; defaults to nonstrict. CLI mode and file directives take precedence."
    )]
    pub mode: Option<crate::analysis::Mode>,

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

    #[schemars(
        default,
        description = "Downloaded Roblox types, instance mappings, and analysis permissions."
    )]
    pub roblox: Option<RobloxConfig>,
}

#[derive(
    Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize, JsonSchema, serde::Serialize,
)]
#[serde(deny_unknown_fields)]
pub struct RobloxConfig {
    #[schemars(
        default,
        description = "Roblox cache directory, relative to this configuration. Defaults to the platform cache directory under instar/roblox."
    )]
    pub cache: Option<PathBuf>,

    #[schemars(
        default,
        description = "Full commit hash of the generated Roblox assets. Unset checks for updates every 24 hours."
    )]
    pub revision: Option<String>,

    /// Rojo project path relative to this configuration.
    #[schemars(default)]
    pub project: Option<PathBuf>,

    /// Existing instance-to-source mapping path relative to this configuration.
    #[schemars(default)]
    pub sourcemap: Option<PathBuf>,

    #[schemars(
        default,
        description = "Roblox API security level. Inherits from ancestor configurations; defaults to PluginSecurity."
    )]
    pub level: Option<RobloxLevel>,
}

#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Deserialize,
    JsonSchema,
    serde::Serialize,
)]
pub enum RobloxLevel {
    None,
    LocalUserSecurity,

    #[default]
    PluginSecurity,

    RobloxScriptSecurity,
}

impl InstarConfig {
    /// # Errors
    /// Returns invalid TOML, unknown fields or field-type errors with TOML spans.
    pub fn parse(text: &str) -> Result<Self, toml_edit::de::Error> {
        toml_edit::de::from_str(text)
    }
}
