use std::{collections::BTreeMap, num::NonZeroUsize};

use figment::{Figment, providers::Serialized};
use regex::Regex;
use serde::{Deserialize, de::DeserializeOwned};

use crate::{
    lint::catalog::RULES,
    settings::lint::{Configuration, Preset, Severity},
};

/// Validated rule policies for one configuration scope.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub(crate) rules: BTreeMap<String, Severity>,
    pub(crate) indent_width: NonZeroUsize,
    pub(crate) unused: Unused,
    pub(crate) ignored: Regex,
    pub(crate) immutable: Immutable,
    pub(crate) globals: BTreeMap<String, String>,
    pub(crate) imports: BTreeMap<String, String>,
}

impl Prepared {
    /// Validate and resolve the complete rule catalog for a configuration scope.
    ///
    /// # Errors
    /// Rejects unknown rules, mismatched settings, invalid parameters, and invalid patterns.
    pub fn new(
        configuration: &Configuration,
        indent_width: NonZeroUsize,
    ) -> Result<Self, Box<figment::Error>> {
        let mut rules: BTreeMap<_, _> = RULES
            .iter()
            .map(|definition| {
                (
                    definition.name.clone(),
                    (definition.is_enabled, Severity::Warning),
                )
            })
            .collect();

        for preset in &configuration.presets {
            for definition in RULES.iter() {
                if let Some((is_enabled, _)) = rules.get_mut(&definition.name) {
                    *is_enabled = match preset {
                        Preset::None => false,
                        Preset::All => true,
                        Preset::Correctness => definition.is_enabled,
                        Preset::Recommended => definition.is_enabled || definition.is_recommended,
                    };
                }
            }
        }

        for (name, settings) in &configuration.rules {
            let (is_enabled, severity) = rules
                .get_mut(name)
                .ok_or_else(|| invalid(format!("Lint rule `{name}` is not recognized.")))?;

            if let Some(is_true) = settings.is_enabled {
                *is_enabled = is_true;
            }

            if let Some(value) = settings.severity {
                *severity = value;
            }

            if let Some(parameters) = &settings.settings {
                if parameters.rule != *name {
                    return Err(invalid(format!(
                        "Settings rule identifier does not match `{name}`."
                    )));
                }

                if !matches!(
                    name.as_str(),
                    "unused_variable"
                        | "prefer_const"
                        | "restricted_globals"
                        | "restricted_module_paths"
                ) && !parameters.parameters.is_empty()
                {
                    return Err(invalid(format!(
                        "Lint rule `{name}` does not accept parameters."
                    )));
                }
            }
        }

        let unused: Unused = parameters(configuration, "unused_variable")?;

        let ignored = Regex::new(&unused.ignore_pattern).map_err(|error| {
            let mut error = invalid(error.to_string());

            error.path = [
                "rules",
                "unused_variable",
                "settings",
                "parameters",
                "ignore_pattern",
            ]
            .map(str::to_owned)
            .into();

            error
        })?;

        let immutable = parameters(configuration, "prefer_const")?;
        let globals: Globals = parameters(configuration, "restricted_globals")?;
        let imports: Imports = parameters(configuration, "restricted_module_paths")?;

        for (name, reason) in globals.names.iter().chain(&imports.paths) {
            if name.trim().is_empty()
                || name.trim() != name
                || name.contains('\0')
                || reason.trim().is_empty()
                || reason.contains('\0')
            {
                return Err(invalid(
                    "Restricted names and paths must be nonempty, have no surrounding whitespace, and contain no NUL characters. Reasons must contain non-whitespace text and no NUL characters.",
                ));
            }
        }

        if globals.names.keys().any(|name| !is_identifier(name)) {
            return Err(invalid(
                "Restricted global name must start with an ASCII letter or underscore and contain only ASCII letters, digits, or underscores.",
            ));
        }

        let rules: BTreeMap<_, _> = rules
            .into_iter()
            .filter_map(|(name, (is_enabled, severity))| is_enabled.then_some((name, severity)))
            .collect();

        Ok(Self {
            rules,
            indent_width,
            unused,
            ignored,
            immutable,
            globals: globals.names,
            imports: imports.paths,
        })
    }

    pub(crate) fn enabled(&self, name: &str) -> Option<Severity> {
        self.rules.get(name).copied()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Unused {
    #[serde(rename = "parameters")]
    pub(crate) should_check_parameters: bool,

    #[serde(rename = "loop_variables")]
    pub(crate) should_check_loop_variables: bool,

    pub(crate) ignore_pattern: String,
}

impl Default for Unused {
    fn default() -> Self {
        Self {
            should_check_parameters: false,
            should_check_loop_variables: false,
            ignore_pattern: "^_".to_owned(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Immutable {
    #[serde(rename = "mutated_tables_stay_local")]
    pub(crate) should_keep_mutated_tables_local: bool,
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

fn parameters<Value>(
    configuration: &Configuration,
    name: &str,
) -> Result<Value, Box<figment::Error>>
where
    Value: Default + DeserializeOwned,
{
    let Some(settings) = configuration
        .rules
        .get(name)
        .and_then(|rule| rule.settings.as_ref())
    else {
        return Ok(Value::default());
    };

    Figment::from(Serialized::defaults(&settings.parameters))
        .extract()
        .map_err(|mut error| {
            error.path = ["rules", name, "settings", "parameters"]
                .into_iter()
                .map(str::to_owned)
                .chain(error.path)
                .collect();

            error.metadata = None;
            error.profile = None;

            Box::new(error)
        })
}

fn invalid(message: impl Into<String>) -> Box<figment::Error> {
    Box::new(figment::Error::from(message.into()))
}

fn is_identifier(name: &str) -> bool {
    let mut bytes = name.bytes();

    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}
