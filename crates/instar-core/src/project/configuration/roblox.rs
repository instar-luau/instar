use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Opt-in Roblox environment and generated-asset selection.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
#[serde(deny_unknown_fields)]
pub struct RobloxConfig {
    #[serde(skip)]
    #[schemars(skip)]
    pub(crate) root: PathBuf,

    /// Rojo project path relative to this configuration.
    #[schemars(default)]
    pub project: Option<PathBuf>,

    /// Existing instance-to-source mapping path relative to this configuration.
    #[schemars(default)]
    pub sourcemap: Option<PathBuf>,

    /// Roblox API security level. Inherits from ancestor configurations; defaults to `PluginSecurity`.
    #[schemars(default)]
    pub level: Option<RobloxLevel>,
}

/// Permission level used to select accessible Roblox API members.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize, JsonSchema,
)]
pub enum RobloxLevel {
    /// Members available to ordinary experience scripts.
    None,

    /// Members additionally available to local user scripts.
    LocalUserSecurity,

    /// Members additionally available to Studio plugins.
    #[default]
    PluginSecurity,

    /// Members additionally available to Roblox internal scripts.
    RobloxScriptSecurity,
}
