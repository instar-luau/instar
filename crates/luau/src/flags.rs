//! Typed immutable Luau flag and resource profiles.

use std::{collections::BTreeMap, io};

use serde::{Deserialize, Serialize};

use crate::boundary;

/// A typed process-global Luau flag value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(untagged)]
pub enum FlagValue {
    /// A boolean scalar.
    Boolean(bool),

    /// A signed 32-bit integer flag value.
    Integer(i32),
}

/// The supported immutable Luau solver profile.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum SolverMode {
    /// The supported new Luau solver.
    New,
}

/// A normalized immutable flag, solver and resource-limit profile.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct LanguageProfile {
    /// Typed normalized language flag selections.
    pub flags: BTreeMap<String, FlagValue>,

    /// The immutable supported solver selection.
    pub solver: SolverMode,

    /// The explicit type-function VM heap limit, not total host allocation bound.
    pub type_function_heap_bytes: u64,
}

impl LanguageProfile {
    /// Captures registry defaults and a positive immutable type-function VM heap limit.
    ///
    /// # Errors
    /// Rejects unknown flags, conflicting aliases, stdout flags and unsupported limits.
    pub fn new(
        flags: &BTreeMap<String, FlagValue>,
        type_function_heap_bytes: u64,
    ) -> io::Result<Self> {
        let limit = i32::try_from(type_function_heap_bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;

        if limit < 1024 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "type-function VM heap limit must be at least one mebibyte",
            ));
        }

        let mut selected = flags.clone();

        if selected
            .get("DebugLuauTypeFunctionRuntimeHeapLimit")
            .is_some_and(|value| *value != FlagValue::Integer(limit))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "conflicting type-function VM heap limit",
            ));
        }

        selected.insert(
            "DebugLuauTypeFunctionRuntimeHeapLimit".to_owned(),
            FlagValue::Integer(limit),
        );

        Ok(Self {
            flags: normalize(&selected)?,
            solver: SolverMode::New,
            type_function_heap_bytes,
        })
    }
}

/// Validates canonical registry names while retaining effective defaults.
///
/// # Errors
/// Rejects unknown names, incorrect types, duplicate aliases and stdout flags.
pub fn normalize(flags: &BTreeMap<String, FlagValue>) -> io::Result<BTreeMap<String, FlagValue>> {
    Ok(boundary::normalize_flags(&native(flags))
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?
        .into_iter()
        .map(|flag| {
            (
                flag.name,
                if flag.is_boolean {
                    FlagValue::Boolean(flag.boolean)
                } else {
                    FlagValue::Integer(flag.integer)
                },
            )
        })
        .collect())
}

pub(crate) fn apply(flags: &BTreeMap<String, FlagValue>) -> io::Result<()> {
    boundary::apply_flags(&native(flags)).map_err(io::Error::other)
}

pub(crate) fn native(flags: &BTreeMap<String, FlagValue>) -> Vec<boundary::NativeFlag> {
    flags
        .iter()
        .map(|(name, value)| boundary::NativeFlag {
            name: name.clone(),
            boolean: matches!(value, FlagValue::Boolean(true)),
            integer: match value {
                FlagValue::Integer(value) => *value,
                FlagValue::Boolean(_) => 0,
            },
            is_boolean: matches!(value, FlagValue::Boolean(_)),
        })
        .collect()
}
