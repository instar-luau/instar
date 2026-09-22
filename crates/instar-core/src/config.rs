//! Project configuration formats and Luau settings.

use std::{collections::BTreeMap, io, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vermis::{Kind, Parts, View};

use crate::{invalid, string_value};

/// Instar project configuration. Luau settings live in `[luau]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Settings shared by analysis and require resolution.
    pub luau: LuauConfig,

    /// Optional Roblox sourcemap configuration.
    pub roblox: RobloxConfig,
}

/// Tool-neutral sourcemap settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RobloxConfig {
    /// Sourcemap files relative to this manifest; their source paths are relative to each map.
    /// Omission inherits the parent list, or discovers the nearest ancestor `sourcemap.json`.
    /// An explicit list replaces it, including `[]` to disable discovery.
    #[schemars(with = "Option<Vec<String>>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sourcemaps: Option<Vec<PathBuf>>,
}

/// Luau settings in Instar's `snake_case` configuration format.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LuauConfig {
    /// Typechecking mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_mode: Option<Mode>,

    /// Warning overrides, including the `*` wildcard.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub lint: BTreeMap<String, bool>,

    /// Whether lint warnings are errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lint_errors: Option<bool>,

    /// Whether type errors are reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_errors: Option<bool>,

    /// Additional globals. An explicit list replaces the inherited list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub globals: Option<Vec<String>>,

    /// Case-insensitive aliases; relative targets retain their defining directory.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,
}

/// Luau typechecking mode.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Skip typechecking.
    Nocheck,

    /// Infer types permissively.
    Nonstrict,

    /// Require strict type correctness.
    Strict,
}

impl LuauConfig {
    pub(super) fn merge(&mut self, layer: &Self) {
        if layer.language_mode.is_some() {
            self.language_mode = layer.language_mode;
        }

        if layer.lint_errors.is_some() {
            self.lint_errors = layer.lint_errors;
        }

        if layer.type_errors.is_some() {
            self.type_errors = layer.type_errors;
        }

        if layer.globals.is_some() {
            self.globals.clone_from(&layer.globals);
        }

        if layer.lint.contains_key("*") {
            self.lint.clear();
        }

        self.lint.extend(layer.lint.clone());
        self.aliases.extend(layer.aliases.clone());
    }

    pub(super) fn validate(&mut self) -> io::Result<()> {
        let mut aliases = BTreeMap::new();

        for (key, value) in std::mem::take(&mut self.aliases) {
            if key.is_empty()
                || key == "."
                || key == ".."
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(invalid(format!("invalid alias {key:?}")));
            }

            let name = key.to_ascii_lowercase();

            if name == "self" {
                return Err(invalid("the alias @self is reserved"));
            }

            if value.is_empty() || value.contains('\0') {
                return Err(invalid(format!("invalid target for alias {key:?}")));
            }

            if aliases.insert(name, value).is_some() {
                return Err(invalid(format!("duplicate case-insensitive alias {key:?}")));
            }
        }

        self.aliases = aliases;
        // Native validation covers warning names and keeps the JSON contract authoritative.
        instar_bridge::Configuration::new(self.native_json()?.as_bytes())?;

        Ok(())
    }

    pub(super) fn native_json(&self) -> io::Result<String> {
        let mut value = serde_json::to_value(self)?;

        if let Value::Object(object) = &mut value {
            for (from, to) in [
                ("language_mode", "languageMode"),
                ("lint_errors", "lintErrors"),
                ("type_errors", "typeErrors"),
            ] {
                if let Some(value) = object.remove(from) {
                    object.insert(to.to_owned(), value);
                }
            }
        }

        serde_json::to_string(&value).map_err(|error| invalid(error.to_string()))
    }
}

pub(super) fn parse_json(source: &str) -> io::Result<LuauConfig> {
    let options = jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };

    let value: Value = jsonc_parser::parse_to_serde_value(source, &options)
        .map_err(|error| invalid(error.to_string()))?;

    let Value::Object(object) = value else {
        return Err(invalid("configuration must be an object"));
    };

    legacy_config(object, ["languageMode", "lintErrors", "typeErrors"])
}

fn legacy_config(mut object: Map<String, Value>, keys: [&str; 3]) -> io::Result<LuauConfig> {
    for (from, to) in keys
        .into_iter()
        .zip(["language_mode", "lint_errors", "type_errors"])
    {
        if object.contains_key(to) {
            return Err(invalid(format!(
                "unknown legacy setting {to:?}; use {from:?}"
            )));
        }

        if let Some(value) = object.remove(from) {
            object.insert(to.to_owned(), value);
        }
    }

    serde_json::from_value(Value::Object(object)).map_err(|error| invalid(error.to_string()))
}

pub(super) fn parse_luau(source: &str) -> io::Result<LuauConfig> {
    let tree = vermis::parse(source.as_bytes());

    if let Some(error) = tree.diagnostics.first() {
        return Err(invalid(format!(
            "{} at byte {}",
            error.message, error.span.start
        )));
    }

    let root = tree
        .view(tree.root)
        .ok_or_else(|| invalid("missing configuration root"))?;

    let Some(Parts::Root { block }) = root.parts() else {
        return Err(invalid("expected configuration block"));
    };

    let mut locals = BTreeMap::new();
    let mut returned = None;

    for statement in block.children() {
        if returned.is_some() {
            return Err(invalid("unexpected statement after config return"));
        }

        match statement.parts() {
            Some(Parts::Local { bindings, values }) => {
                let values = values
                    .map(|value| literal(value, &locals))
                    .collect::<io::Result<Vec<_>>>()?;

                for (index, binding) in bindings.enumerate() {
                    let Some(Parts::Binding { name, .. }) = binding.parts() else {
                        return Err(invalid("invalid config binding"));
                    };

                    let name =
                        std::str::from_utf8(name.text()).map_err(|e| invalid(e.to_string()))?;

                    locals.insert(
                        name.to_owned(),
                        values.get(index).cloned().unwrap_or(Value::Null),
                    );
                }
            }

            Some(Parts::Return { mut values }) => {
                let value = values
                    .next()
                    .ok_or_else(|| invalid("config must return a table"))?;

                if values.next().is_some() {
                    return Err(invalid("config must return exactly one table"));
                }

                returned = Some(literal(value, &locals)?);
            }

            _ => {
                return Err(invalid(format!(
                    "configuration must be declarative; unsupported statement at byte {}",
                    statement.span().start
                )));
            }
        }
    }

    let value = returned.ok_or_else(|| invalid("configuration must return a table"))?;

    let object = value
        .as_object()
        .ok_or_else(|| invalid("configuration must return a table"))?;

    let Some(luau) = object.get("luau") else {
        return Ok(LuauConfig::default());
    };

    let mut luau = luau
        .as_object()
        .ok_or_else(|| invalid("luau must be a table"))?
        .clone();

    if luau
        .get("globals")
        .is_some_and(|v| v.as_object().is_some_and(Map::is_empty))
    {
        luau.insert("globals".to_owned(), Value::Array(Vec::new()));
    }

    legacy_config(luau, ["languagemode", "linterrors", "typeerrors"])
}

fn literal(node: View<'_, '_>, locals: &BTreeMap<String, Value>) -> io::Result<Value> {
    match node.kind() {
        Kind::String => return string_value(node.text()).map(Value::String),
        Kind::Boolean => return Ok(Value::Bool(node.text() == b"true")),
        Kind::Nil => return Ok(Value::Null),

        Kind::Name => {
            let name = std::str::from_utf8(node.text()).map_err(|e| invalid(e.to_string()))?;

            return locals
                .get(name)
                .cloned()
                .ok_or_else(|| invalid(format!("unknown config constant {name}")));
        }

        Kind::Number => {
            let text = std::str::from_utf8(node.text()).map_err(|e| invalid(e.to_string()))?;

            return serde_json::from_str(text).map_err(|e| invalid(e.to_string()));
        }

        _ => {}
    }

    match node.parts() {
        Some(Parts::Group { expression } | Parts::Assertion { expression, .. }) => {
            literal(expression, locals)
        }

        Some(Parts::Binary {
            left,
            operator,
            right,
        }) if operator.text() == b".." => {
            let left = literal(left, locals)?;
            let right = literal(right, locals)?;

            match (left, right) {
                (Value::String(left), Value::String(right)) => Ok(Value::String(left + &right)),
                _ => Err(invalid("config concatenation requires strings")),
            }
        }

        Some(Parts::Table { fields }) => {
            let mut object = Map::new();
            let mut array = BTreeMap::new();
            let mut next_index = 1_u64;

            for field in fields {
                let Some(Parts::TableField {
                    key,
                    value,
                    indexed,
                }) = field.parts()
                else {
                    return Err(invalid("invalid config table field"));
                };

                let value = literal(value, locals)?;

                let key = match key {
                    Some(key) if !indexed => Value::String(
                        String::from_utf8(key.text().to_vec())
                            .map_err(|e| invalid(e.to_string()))?,
                    ),

                    Some(key) => literal(key, locals)?,

                    None => {
                        let key = Value::from(next_index);
                        next_index += 1;

                        key
                    }
                };

                match key {
                    Value::String(key) => {
                        object.insert(key, value);
                    }

                    Value::Number(key) => {
                        let key = key.as_u64().filter(|&n| n > 0).ok_or_else(|| {
                            invalid("config array index must be a positive integer")
                        })?;

                        array.insert(key, value);
                    }

                    _ => {
                        return Err(invalid(
                            "config table key must be a string or positive integer",
                        ));
                    }
                }
            }

            if array.is_empty() {
                return Ok(Value::Object(object));
            }

            if !object.is_empty() {
                return Err(invalid("mixed config tables are not supported"));
            }

            if array
                .keys()
                .copied()
                .ne(1..=u64::try_from(array.len()).map_err(|e| invalid(e.to_string()))?)
            {
                return Err(invalid("config arrays must be contiguous"));
            }

            Ok(Value::Array(array.into_values().collect()))
        }

        _ => Err(invalid(format!(
            "configuration must be declarative; unsupported expression at byte {}",
            node.span().start
        ))),
    }
}
