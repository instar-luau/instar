/// Formatter settings and layout policies.
pub mod format;

mod schema;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize, de::Error};
use std::{collections::BTreeMap, path::PathBuf};

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
    pub build: Option<crate::build::configuration::Settings>,

    /// Lint rules, groups, globals, selection, and options. Settings inherit from ancestor configurations.
    #[schemars(default)]
    pub lint: Option<crate::lint::configuration::Settings>,

    /// Type checking, external definitions and documentation, aliases, Roblox integration, and source selection.
    #[schemars(default)]
    pub analyze: Option<AnalyzeConfig>,

    /// Formatter settings. Ancestor settings merge field by field; absent settings use their documented defaults.
    #[schemars(default)]
    pub format: Option<format::Options>,

    /// Graft projects loaded from local paths or installed GitHub releases. Entries inherit by name and run in name order.
    #[schemars(default)]
    pub grafts: Option<BTreeMap<String, crate::graft::Dependency>>,

    /// Identity, protocol, runtime, and hooks exported by this graft project.
    #[schemars(default)]
    pub graft: Option<crate::graft::Manifest>,

    /// Source selection patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub include: Option<Vec<String>>,

    /// Source exclusion patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub exclude: Option<Vec<String>>,
}

/// Type analysis settings inherited from ancestor configurations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AnalyzeConfig {
    /// Source patterns included only during analysis.
    pub include: Option<Vec<String>>,

    /// Source patterns excluded only during analysis.
    pub exclude: Option<Vec<String>>,

    /// Checking mode. CLI mode and file directives take precedence.
    pub mode: Option<crate::analysis::Mode>,

    /// External Luau definition files resolved relative to this configuration.
    pub definitions: Option<Vec<PathBuf>>,

    /// External documentation files resolved relative to this configuration.
    pub documentation: Option<Vec<PathBuf>>,

    /// Require aliases mapped to paths relative to this configuration.
    pub aliases: Option<BTreeMap<String, PathBuf>>,

    /// Roblox types, instance mappings, and analysis permissions.
    pub roblox: Option<RobloxConfig>,
}

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

        if let Some(manifest) = &configuration.graft {
            manifest.validate().map_err(toml_edit::de::Error::custom)?;
        }

        Ok(configuration)
    }
}
