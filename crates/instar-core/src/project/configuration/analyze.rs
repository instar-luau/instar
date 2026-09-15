use super::RobloxConfig;
use crate::analysis::Mode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Type analysis settings inherited from ancestor configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AnalyzeConfig {
    /// Source patterns included only during analysis.
    pub include: Option<Vec<String>>,

    /// Source patterns excluded only during analysis.
    pub exclude: Option<Vec<String>>,

    /// Checking mode. CLI mode and file directives take precedence.
    pub mode: Option<Mode>,

    /// External Luau definition files resolved relative to this configuration.
    pub definitions: Option<Vec<PathBuf>>,

    /// External documentation files resolved relative to this configuration.
    pub documentation: Option<Vec<PathBuf>>,

    /// Require aliases mapped to paths relative to this configuration.
    pub aliases: Option<std::collections::BTreeMap<String, PathBuf>>,

    /// Roblox types, instance mappings, and analysis permissions.
    pub roblox: Option<RobloxConfig>,
}
