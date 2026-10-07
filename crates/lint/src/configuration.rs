//! Instar rule configuration and validation.
use std::{collections::BTreeMap, num::NonZeroUsize};

use instar_analysis::error::invalid;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::Rule;

macro_rules! configuration {
    ($($field:ident => $identity:ident: $settings:ident, $documentation:literal;)*) => {
        /// Effective Instar linter settings after configuration inheritance.
        #[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct Configuration {
            /// Included entry patterns, inheriting global selection when omitted.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub include: Option<Vec<String>>,
            /// Excluded entry patterns, inheriting global selection when omitted.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub exclude: Option<Vec<String>>,
            $(#[doc = $documentation] pub $field: $settings,)*
        }

        impl Configuration {
            /// Returns every configured rule and its effective severity.
            #[must_use]
            pub fn rules(&self) -> Vec<(Rule, Level)> {
                vec![$((Rule::$identity, self.$field.level),)*]
            }

            /// Returns the severity for an inventoried rule.
            #[must_use]
            pub const fn level(&self, rule: Rule) -> Level {
                match rule { $(Rule::$identity => self.$field.level,)* }
            }
        }
    };
}

/// Severity of an Instar lint rule.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Do not report findings.
    Allow,

    /// Report informational findings.
    Info,

    /// Report warnings.
    #[default]
    Warn,

    /// Report errors.
    Deny,
}

/// Settings for a rule enabled at warning severity by default.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// Severity of findings.
    pub level: Level,
}

/// Settings for a rule allowed by default.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AllowedSettings {
    /// Severity of findings.
    pub level: Level,
}

impl Default for AllowedSettings {
    fn default() -> Self {
        Self {
            level: Level::Allow,
        }
    }
}

crate::inventory::inventory!(configuration);

impl Configuration {
    /// Whether inferred native binding types are needed.
    #[must_use]
    pub fn semantic(&self) -> bool {
        self.implicit_any_local.level != Level::Allow
            || self.implicit_any_parameter.level != Level::Allow
    }

    /// Validates configuration and returns the compiled ignored-name expression.
    ///
    /// # Errors
    /// Returns an error for invalid include or exclude globs or an invalid
    /// unused-variable ignored-name regular expression.
    pub fn validate(&self) -> std::io::Result<regex::Regex> {
        instar_analysis::selection::validate(self.include.as_deref(), "lint.include")?;
        instar_analysis::selection::validate(self.exclude.as_deref(), "lint.exclude")?;

        regex::Regex::new(&self.unused_variable.ignore_pattern).map_err(|error| {
            invalid(format!(
                "lint.unused_variable.ignore_pattern: invalid regular expression: {error}"
            ))
        })
    }
}

/// Unused binding rule settings.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnusedVariable {
    /// Severity of findings.
    pub level: Level,

    /// Include unused parameters.
    pub parameters: bool,

    /// Include unused loop variables.
    pub loop_variables: bool,

    /// Regular expression matching names exempt from the rule.
    pub ignore_pattern: String,
}

impl Default for UnusedVariable {
    fn default() -> Self {
        Self {
            level: Level::Warn,
            parameters: false,
            loop_variables: false,
            ignore_pattern: "^_".into(),
        }
    }
}

/// Cyclomatic complexity rule settings.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HighCyclomaticComplexity {
    /// Severity of findings.
    pub level: Level,

    /// Largest permitted cyclomatic complexity.
    pub maximum_complexity: NonZeroUsize,
}

impl Default for HighCyclomaticComplexity {
    fn default() -> Self {
        Self {
            level: Level::Allow,
            maximum_complexity: NonZeroUsize::new(40).expect("40 is nonzero"),
        }
    }
}

/// Immutable binding preference settings.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PreferConst {
    /// Severity of findings.
    pub level: Level,

    /// Keep locally mutated tables as local bindings.
    pub mutated_tables_stay_local: bool,
}

impl Default for PreferConst {
    fn default() -> Self {
        Self {
            level: Level::Allow,
            mutated_tables_stay_local: false,
        }
    }
}

/// Project-specific deprecation rule settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Deprecated {
    /// Severity of findings.
    pub level: Level,

    /// Deprecated symbol paths mapped to suggested replacements.
    pub paths: BTreeMap<String, String>,

    /// Report ambiguous method matches.
    pub ambiguous_methods: bool,
}

/// Restricted global rule settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedGlobals {
    /// Severity of findings.
    pub level: Level,

    /// Restricted global names mapped to reasons.
    pub names: BTreeMap<String, String>,
}

/// Restricted module path rule settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedModulePaths {
    /// Severity of findings.
    pub level: Level,

    /// Restricted module paths mapped to reasons.
    pub paths: BTreeMap<String, String>,
}
