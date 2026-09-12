use super::registry;
use crate::{configuration::InstarConfig, source::SourceStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Allow,
    Info,
    Warn,
    Deny,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintSettings")]
pub struct Settings {
    #[schemars(description = "Enable linting. Unset enables linting.")]
    pub enabled: Option<bool>,

    #[schemars(
        description = "Group levels. Disabled-by-default rules require an individual rule override."
    )]
    pub groups: BTreeMap<String, Level>,

    #[schemars(description = "Individual rule levels. Graft rules use graft/rule identifiers.")]
    pub rules: BTreeMap<String, Level>,

    #[schemars(description = "Additional global names available to this project.")]
    pub globals: Vec<String>,

    #[schemars(description = "Options for configurable rules.")]
    pub options: Options,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintOptions")]
pub struct Options {
    #[schemars(description = "Shared options for unused bindings, functions, and imports.")]
    pub unused_variable: Unused,

    #[schemars(description = "Additional deprecated functions and replacements.")]
    pub deprecated_function: Deprecated,

    #[schemars(description = "Restricted module paths and reasons.")]
    pub restricted_import: Restricted,

    #[schemars(description = "Restricted global names mapped to reasons.")]
    pub restricted_global: BTreeMap<String, String>,

    #[schemars(description = "Function complexity limits.")]
    pub function_complexity: Complexity,

    #[schemars(description = "Constant binding preferences.")]
    pub constant_binding: Constant,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintUnused")]
pub struct Unused {
    #[schemars(description = "Report unused function parameters.")]
    pub parameters: bool,

    #[schemars(description = "Report unused loop variables.")]
    pub loop_variables: bool,

    #[schemars(description = "Regular expression matching ignored binding names.")]
    pub ignore_pattern: String,
}
impl Default for Unused {
    fn default() -> Self {
        Self {
            parameters: bool::default(),
            loop_variables: bool::default(),
            ignore_pattern: "^_".into(),
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Deprecated {
    #[schemars(description = "Deprecated function names mapped to replacement descriptions.")]
    pub additional: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Restricted {
    #[schemars(description = "Exact module paths mapped to restriction reasons.")]
    pub paths: BTreeMap<String, String>,
}
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Complexity {
    #[schemars(description = "Maximum permitted cyclomatic complexity.")]
    pub maximum_complexity: u32,
}
impl Default for Complexity {
    fn default() -> Self {
        Self {
            maximum_complexity: 40,
        }
    }
}
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Constant {
    #[schemars(description = "Keep local for bindings whose table fields are mutated.")]
    pub mutated_tables_stay_local: bool,
}

impl Settings {
    #[must_use]
    pub fn level(&self, name: &str) -> Level {
        if self.enabled == Some(false) {
            return Level::Allow;
        }

        if let Some(level) = self.rules.get(name) {
            return *level;
        }

        let Some(rule) = registry::find(name) else {
            return Level::Warn;
        };

        if rule.level == Level::Allow {
            return Level::Allow;
        }

        self.groups.get(rule.group).copied().unwrap_or(rule.level)
    }

    /// # Errors
    /// Returns unknown rule, group, or invalid option errors.
    pub fn validate(&self) -> io::Result<()> {
        for name in self.rules.keys() {
            if registry::find(name).is_none() && !name.contains('/') {
                return Err(io::Error::other(format!("unknown lint rule: {name}")));
            }
        }

        for name in self.groups.keys() {
            if !registry::RULES.iter().any(|rule| rule.group == name) {
                return Err(io::Error::other(format!("unknown lint group: {name}")));
            }
        }

        crate::luau::matches(&self.options.unused_variable.ignore_pattern, "")?;

        Ok(())
    }
}

pub(crate) struct Configuration {
    pub settings: Settings,
    pub grafts: Vec<(String, crate::graft::Graft)>,
}

pub(crate) fn discover(sources: &mut SourceStore, path: &Path) -> io::Result<Configuration> {
    let mut merged = serde_json::Map::new();
    let mut grafts = BTreeMap::<String, PathBuf>::new();

    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("source has no parent"))?;

    for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let configuration = ancestor.join("instar.toml");

        if !sources.is_open(&configuration).map_err(io::Error::other)? {
            match std::fs::metadata(&configuration) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
                Ok(_) => {}
            }
        }

        let source = sources.read(&configuration).map_err(io::Error::other)?;
        let text = source.text().map_err(io::Error::other)?;
        InstarConfig::parse(text).map_err(io::Error::other)?;

        let mut value: serde_json::Value =
            toml_edit::de::from_str(text).map_err(io::Error::other)?;

        if let Some(entries) = value["grafts"].as_object() {
            for (name, path) in entries {
                grafts.insert(
                    name.clone(),
                    ancestor.join(
                        path.as_str()
                            .ok_or_else(|| io::Error::other("invalid graft path"))?,
                    ),
                );
            }
        }

        if let serde_json::Value::Object(value) = value["lint"].take() {
            overlay(&mut merged, value);
        }
    }

    let settings: Settings = serde_json::from_value(merged.into()).map_err(io::Error::other)?;
    settings.validate()?;

    Ok(Configuration {
        settings,
        grafts: grafts
            .into_iter()
            .map(|(name, path)| crate::graft::Graft::load(&path, &name).map(|graft| (name, graft)))
            .collect::<io::Result<_>>()?,
    })
}

fn overlay(
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
