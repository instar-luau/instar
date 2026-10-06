//! Typed overrides for registered native Luau flags.

use std::{collections::BTreeMap, io};

use serde::{Deserialize, Serialize};

use crate::boundary;

/// One registered boolean flag or integer limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Value {
    /// Boolean fast flag.
    Boolean(bool),

    /// Integer fast limit.
    Integer(i32),
}

pub(crate) fn native(flags: &BTreeMap<String, Value>) -> Vec<boundary::NativeFlag> {
    flags
        .iter()
        .map(|(name, value)| boundary::NativeFlag {
            name: name.clone(),
            boolean: matches!(value, Value::Boolean(true)),
            integer: match value {
                Value::Integer(value) => *value,
                Value::Boolean(_) => 0,
            },
            is_boolean: matches!(value, Value::Boolean(_)),
        })
        .collect()
}

/// Validates flag names and types without modifying native process state.
///
/// # Errors
/// Rejects unknown registry names and mismatched value types.
pub fn validate(flags: &BTreeMap<String, Value>) -> io::Result<()> {
    boundary::validate_flags(&native(flags)).map_err(io::Error::other)
}

pub(crate) fn normalize(flags: &BTreeMap<String, Value>) -> io::Result<BTreeMap<String, Value>> {
    Ok(boundary::normalize_flags(&native(flags))
        .map_err(io::Error::other)?
        .into_iter()
        .map(|flag| {
            (
                flag.name,
                if flag.is_boolean {
                    Value::Boolean(flag.boolean)
                } else {
                    Value::Integer(flag.integer)
                },
            )
        })
        .collect())
}

pub(crate) fn apply(flags: &BTreeMap<String, Value>) -> io::Result<()> {
    boundary::apply_flags(&native(flags)).map_err(io::Error::other)
}
