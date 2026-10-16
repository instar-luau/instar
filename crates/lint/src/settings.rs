use std::collections::{BTreeMap, BTreeSet};

use figment::{Figment, providers::Serialized};
use instar_configuration::linting::{Configuration, Preset, Severity};
use regex::Regex;
use serde::{Deserialize, de::DeserializeOwned};

use crate::catalog::RULES;

/// Validated rule policies for one configuration scope.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub(crate) rules: BTreeMap<String, Policy>,
    pub(crate) unused: Unused,
    pub(crate) ignored: Regex,
    pub(crate) immutable: Immutable,
    pub(crate) deprecated: Deprecated,
    pub(crate) globals: BTreeMap<String, String>,
    pub(crate) imports: BTreeMap<String, String>,
}

#[derive(Clone, Debug)]
pub(crate) struct Policy {
    pub(crate) enabled: bool,
    pub(crate) severity: Severity,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Unused {
    pub(crate) parameters: bool,
    pub(crate) loop_variables: bool,
    pub(crate) ignore_pattern: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Immutable {
    pub(crate) mutated_tables_stay_local: bool,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Deprecated {
    pub(crate) paths: BTreeMap<String, String>,
    pub(crate) ambiguous_methods: bool,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Globals {
    names: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Imports {
    paths: BTreeMap<String, String>,
}

impl Default for Unused {
    fn default() -> Self {
        Self {
            parameters: false,
            loop_variables: false,
            ignore_pattern: "^_".to_owned(),
        }
    }
}

impl Prepared {
    /// Validate and resolve the complete rule catalog for a configuration scope.
    ///
    /// # Errors
    /// Rejects unknown rules, mismatched settings, invalid parameters, and invalid patterns.
    pub fn new(configuration: &Configuration) -> Result<Self, Box<figment::Error>> {
        let mut rules: BTreeMap<_, _> = RULES
            .iter()
            .map(|definition| {
                (
                    definition.name.clone(),
                    Policy {
                        enabled: definition.enabled,
                        severity: if definition.upstream && configuration.upstream_errors {
                            Severity::Error
                        } else {
                            Severity::Warning
                        },
                    },
                )
            })
            .collect();

        for preset in &configuration.presets {
            for definition in RULES.iter() {
                if let Some(policy) = rules.get_mut(&definition.name) {
                    policy.enabled = match preset {
                        Preset::None => false,
                        Preset::All => true,
                        Preset::Correctness => definition.enabled,
                        Preset::Recommended => definition.enabled || definition.recommended,
                    };
                }
            }
        }

        for (name, settings) in &configuration.rules {
            let policy = rules
                .get_mut(name)
                .ok_or_else(|| invalid(format!("unknown lint rule {name}")))?;

            if let Some(enabled) = settings.enabled {
                policy.enabled = enabled;
            }

            if let Some(severity) = settings.severity {
                policy.severity = severity;
            }

            if let Some(parameters) = &settings.settings {
                if parameters.rule != *name {
                    return Err(invalid(format!("settings identity does not match {name}")));
                }

                if !matches!(
                    name.as_str(),
                    "unused_variable"
                        | "prefer_const"
                        | "deprecated"
                        | "restricted_globals"
                        | "restricted_module_paths"
                ) && !parameters.parameters.is_empty()
                {
                    return Err(invalid(format!("{name} does not accept parameters")));
                }
            }
        }

        if let Some(selected) = &configuration.selected {
            for name in selected {
                if !rules.contains_key(name) {
                    return Err(invalid(format!("unknown lint rule {name}")));
                }
            }

            let selected: BTreeSet<_> = selected.iter().collect();

            for (name, policy) in &mut rules {
                policy.enabled = selected.contains(name);
            }
        }

        let unused: Unused = parameters(configuration, "unused_variable")?;

        let ignored = Regex::new(&unused.ignore_pattern)
            .map_err(|error| invalid(format!("unused_variable.ignore_pattern: {error}")))?;

        let immutable = parameters(configuration, "prefer_const")?;
        let deprecated: Deprecated = parameters(configuration, "deprecated")?;
        let globals: Globals = parameters(configuration, "restricted_globals")?;
        let imports: Imports = parameters(configuration, "restricted_module_paths")?;

        for (name, replacement) in &deprecated.paths {
            if !name.split('.').all(identifier)
                || replacement.trim().is_empty()
                || replacement.contains('\0')
            {
                return Err(invalid(format!(
                    "invalid deprecated symbol or replacement: {name}"
                )));
            }
        }

        for (name, reason) in globals.names.iter().chain(&imports.paths) {
            if name.trim().is_empty()
                || name.trim() != name
                || name.contains('\0')
                || reason.trim().is_empty()
                || reason.contains('\0')
            {
                return Err(invalid("restricted names and reasons must be nonempty"));
            }
        }

        if globals.names.keys().any(|name| !identifier(name)) {
            return Err(invalid("invalid restricted global name"));
        }

        Ok(Self {
            rules,
            unused,
            ignored,
            immutable,
            deprecated,
            globals: globals.names,
            imports: imports.paths,
        })
    }

    pub(crate) fn enabled(&self, name: &str) -> Option<&Policy> {
        self.rules.get(name).filter(|policy| policy.enabled)
    }
}

fn parameters<Value: Default + DeserializeOwned>(
    configuration: &Configuration,
    name: &str,
) -> Result<Value, Box<figment::Error>> {
    let Some(settings) = configuration
        .rules
        .get(name)
        .and_then(|rule| rule.settings.as_ref())
    else {
        return Ok(Value::default());
    };

    Figment::from(Serialized::defaults(&settings.parameters))
        .extract()
        .map_err(|error| invalid(format!("{name}: {error}")))
}

fn invalid(message: impl Into<String>) -> Box<figment::Error> {
    Box::new(figment::Error::from(message.into()))
}

fn identifier(name: &str) -> bool {
    let mut bytes = name.bytes();

    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
