//! Lint selection and rule configuration.

use std::{collections::BTreeMap, mem};

use figment::value::{Dict, Value};
use schemars::{JsonSchema, Schema, SchemaGenerator, json_schema};
use serde::{Deserialize, Serialize};

use super::{Patterns, schema};

/// Lint settings and file selection.
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Replacement inclusion patterns.
    pub include: Option<Patterns>,

    /// Replacement exclusion patterns.
    pub exclude: Option<Patterns>,

    /// Presets applied in order over the rule catalog's defaults.
    pub presets: Vec<Preset>,

    /// Named rule overrides, merged by rule and parameter name.
    #[schemars(schema_with = "rules", extend("propertyNames" = schema::identifier()))]
    pub rules: BTreeMap<String, Rule>,
}

/// Named rule-selection presets.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
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
#[derive(Clone, Debug, Default, Deserialize, JsonSchema, Serialize)]
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
#[derive(Clone, Debug, Deserialize, JsonSchema, Serialize)]
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
#[derive(Copy, Clone, Debug, Eq, PartialEq, Deserialize, JsonSchema, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Error diagnostic.
    Error,

    /// Warning diagnostic.
    #[serde(rename = "warn")]
    Warning,

    /// Informational diagnostic.
    #[serde(rename = "info")]
    Information,

    /// Hint diagnostic.
    Hint,
}

pub(super) fn normalize(values: &mut Dict) {
    let Some(Value::Dict(_, lint)) = values.get_mut("lint") else {
        return;
    };

    let Some(Value::Dict(_, rules)) = lint.get_mut("rules") else {
        return;
    };

    for rule in rules.values_mut() {
        if let Value::String(tag, severity) = rule {
            *rule = Value::Dict(
                *tag,
                Dict::from([
                    ("enabled".to_owned(), Value::Bool(*tag, true)),
                    (
                        "severity".to_owned(),
                        Value::String(*tag, mem::take(severity)),
                    ),
                ]),
            );
        }
    }
}

fn rules(generator: &mut SchemaGenerator) -> Schema {
    let mut schema = BTreeMap::<String, Rule>::json_schema(generator);

    schema.insert(
        "additionalProperties".to_owned(),
        json_schema!({
            "anyOf": [
                generator.subschema_for::<Severity>(),
                generator.subschema_for::<Rule>()
            ]
        })
        .into(),
    );

    schema
}
