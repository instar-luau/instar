use schemars::{Schema, generate::SchemaSettings};
use serde_json::{Value, json};

use super::Configuration;

/// Generate the configuration schema accepted by this version of Instar.
#[must_use]
pub fn schema() -> Schema {
    SchemaSettings::draft07()
        .into_generator()
        .into_root_schema_for::<Configuration>()
}

pub(super) fn identifier() -> Value {
    json!({
        "type": "string",
        "pattern": "^[A-Za-z_][A-Za-z0-9_]*$",
        "not": {
            "enum": [
                "and", "break", "do", "else", "elseif", "end", "false", "for",
                "function", "if", "in", "local", "nil", "not", "or", "repeat",
                "return", "then", "true", "until", "while"
            ]
        }
    })
}

pub(super) fn alias() -> Value {
    json!({
        "type": "string",
        "pattern": "^[A-Za-z0-9._-]+$",
        "not": {
            "anyOf": [
                { "enum": [".", ".."] },
                { "pattern": "^[sS][eE][lL][fF]$" }
            ]
        }
    })
}

pub(super) fn pattern() -> Value {
    json!({
        "type": "string",
        "allOf": [
            { "pattern": "\\S" },
            { "pattern": "^[^\\u0000]+$" }
        ]
    })
}

pub(super) fn path() -> Value {
    json!({
        "type": "string",
        "pattern": "^[^\\u0000]+$",
        "not": {
            "anyOf": [
                { "pattern": "://" },
                { "pattern": "^[A-Za-z]:(?:[^/\\\\]|$)" }
            ]
        }
    })
}
