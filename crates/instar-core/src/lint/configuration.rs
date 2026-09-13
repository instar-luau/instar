use super::registry;
use crate::{configuration::InstarConfig, source::SourceStore};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
/// Reporting level assigned to a lint rule.
pub enum Level {
    /// Suppress the finding.
    Allow,

    /// Report an informational finding.
    Info,

    /// Report a warning without failing the command.
    Warn,

    /// Report an error that fails the command.
    Deny,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintSettings")]
/// Lint enablement, severity overrides, globals, and rule options.
pub struct Settings {
    /// Enable linting. Unset enables linting.
    pub enabled: Option<bool>,

    /// Source patterns included only during linting.
    pub include: Vec<String>,

    /// Source patterns excluded only during linting.
    pub exclude: Vec<String>,

    /// Group levels. Disabled-by-default rules require an individual rule override.
    pub groups: BTreeMap<String, Level>,

    /// Individual rule levels. Graft rules use graft/rule identifiers.
    pub rules: BTreeMap<String, Level>,

    /// Additional global names available to this project.
    pub globals: Vec<String>,

    /// Options for configurable rules.
    pub options: Options,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintOptions")]
/// Options shared by configurable built-in lint rules.
pub struct Options {
    /// Shared options for unused bindings, functions, and imports.
    pub unused_variable: Unused,

    /// Additional deprecated functions and replacements.
    pub deprecated_function: Deprecated,

    /// Restricted module paths and reasons.
    pub restricted_import: Restricted,

    /// Restricted global names mapped to reasons.
    pub restricted_global: BTreeMap<String, String>,

    /// Function complexity limits.
    pub function_complexity: Complexity,

    /// Constant binding preferences.
    pub constant_binding: Constant,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
#[schemars(rename = "LintUnused")]
/// Binding categories and names checked for unused declarations.
pub struct Unused {
    /// Report unused function parameters.
    pub parameters: bool,

    /// Report unused loop variables.
    pub loop_variables: bool,

    /// Regular expression matching ignored binding names.
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
/// Project-specific deprecated function declarations.
pub struct Deprecated {
    /// Deprecated function names mapped to replacement descriptions.
    pub additional: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Modules whose imports should produce a lint finding.
pub struct Restricted {
    /// Exact module paths mapped to restriction reasons.
    pub paths: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Threshold for function control-flow complexity.
pub struct Complexity {
    /// Maximum permitted cyclomatic complexity.
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
/// Treatment of table mutation when suggesting constant bindings.
pub struct Constant {
    /// Keep local for bindings whose table fields are mutated.
    pub mutated_tables_stay_local: bool,
}

impl Settings {
    /// Resolve an individual override, group override, or registered default.
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
    let mut grafts = BTreeMap::new();

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
        let parsed = InstarConfig::parse(text).map_err(io::Error::other)?;

        let mut value: serde_json::Value =
            toml_edit::de::from_str(text).map_err(io::Error::other)?;

        for (name, dependency) in parsed.grafts.unwrap_or_default() {
            grafts.insert(name, (dependency, ancestor.to_owned()));
        }

        if let serde_json::Value::Object(value) = value["lint"].take() {
            crate::configuration::overlay(&mut merged, value);
        }
    }

    let settings: Settings = serde_json::from_value(merged.into()).map_err(io::Error::other)?;
    settings.validate()?;

    Ok(Configuration {
        settings,
        grafts: grafts
            .into_iter()
            .map(|(name, (dependency, directory))| {
                crate::graft::Graft::load_with_configuration(
                    &dependency.resolve(&directory, &name)?,
                    &name,
                    dependency.configuration(),
                )
                .map(|graft| (name, graft))
            })
            .collect::<io::Result<_>>()?,
    })
}
