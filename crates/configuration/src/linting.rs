//! Lint selection and rule configuration.

use std::collections::BTreeMap;

use figment::value::Value;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::schema;

/// Lint settings and file selection.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Replacement inclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub include: Option<Vec<String>>,

    /// Replacement exclusion patterns.
    #[schemars(extend("items" = schema::pattern()))]
    pub exclude: Option<Vec<String>>,

    /// Presets applied in order over the rule catalog's defaults.
    pub presets: Vec<Preset>,

    /// Named rule overrides, merged by rule and parameter name.
    #[schemars(extend("propertyNames" = schema::identifier()))]
    pub rules: BTreeMap<String, Rule>,
}

/// Named rule-selection presets.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Correctness rules.
    Correctness,

    /// Recommended rules.
    Recommended,

    /// Every rule.
    All,

    /// No rules.
    None,
}

/// A rule's explicit overrides, retaining omitted catalog defaults.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    /// Enablement override.
    pub enabled: Option<bool>,

    /// Diagnostic severity override.
    pub severity: Option<Severity>,

    /// Rule-owned parameter input.
    pub settings: Option<Settings>,
}

/// Rule-specific parameter input.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Rule name.
    #[schemars(extend("allOf" = [schema::identifier()]))]
    pub rule: String,

    /// Rule-specific parameter table.
    #[serde(default)]
    #[schemars(with = "BTreeMap<String, serde_json::Value>")]
    pub parameters: BTreeMap<String, Value>,
}

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Error diagnostic.
    Error,

    /// Warning diagnostic.
    Warning,

    /// Informational diagnostic.
    Information,

    /// Hint diagnostic.
    Hint,
}
