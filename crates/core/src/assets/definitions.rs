use std::collections::{BTreeMap, BTreeSet};

use instar_bridge::Builtin;
use serde_json::{Value, json};

use super::{
    Error,
    creator::Entry,
    generation::interrupted,
    mapping::{self, Corrections, Dump, Type, invalid},
    syntax::{self, Block, Replacement},
};

#[derive(Default)]
pub(super) struct Catalog {
    pub(super) data_types: Vec<Value>,
    pub(super) constructors: Vec<Value>,
    pub(super) globals: Vec<Value>,
    pub(super) replacements: BTreeMap<String, Vec<Replacement>>,
}

pub(super) fn collect(
    entries: &BTreeMap<String, Entry>,
    dump: &Dump,
    corrections: &Corrections,
    declarations: &[Block],
    imported: &[Block],
    builtins: &[Builtin],
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Catalog, Error> {
    let mut result = Catalog {
        replacements: syntax::replacements(
            include_str!("../../assets/constructors.d.luau"),
            is_cancelled,
        )?
        .into_iter()
        .collect(),
        ..Catalog::default()
    };

    let enumerations = dump
        .enumerations
        .iter()
        .map(|value| value.name.as_str())
        .collect();

    for (path, entry) in entries {
        interrupted(is_cancelled)?;

        if path.starts_with("datatypes/") {
            if matches!(entry.name.as_str(), "Enum" | "EnumItem" | "Enums") {
                continue;
            }

            let mut members = Vec::new();
            let mut constructors = Vec::new();
            let constants = children(entry, "constants")?;

            for group in [
                "properties",
                "methods",
                "constructors",
                "constants",
                "functions",
            ] {
                for member in children(entry, group)? {
                    interrupted(is_cancelled)?;

                    if group == "properties"
                        && constants.iter().any(|value| value.name == member.name)
                    {
                        continue;
                    }

                    let value = convert(&member, group, &entry.name, &enumerations, corrections)?;

                    if matches!(group, "properties" | "methods") {
                        members.push(value);
                    } else {
                        constructors.push(value);
                    }
                }
            }

            for operation in children(entry, "math_operations")? {
                let operand = operation.fields.get("type_b").and_then(Value::as_str);

                let name = match field(&operation, "operation")? {
                    "+" => "__add",
                    "-" if operand.is_none() => "__unm",
                    "-" => "__sub",
                    "*" => "__mul",
                    "/" => "__div",
                    "//" => "__idiv",

                    value => {
                        return Err(invalid(format!(
                            "Unsupported datatype operation `{value}`."
                        )));
                    }
                };

                let parameters = operand
                    .map(|value| {
                        value_type(value, &enumerations, corrections)
                            .map(|value| json!({"Name": "other", "Type": value}))
                    })
                    .transpose()?
                    .into_iter()
                    .collect::<Vec<Value>>();

                members.push(json!({
                    "Name": name,
                    "MemberType": "Function",
                    "Parameters": parameters,
                    "ReturnType": value_type(field(&operation, "return_type")?, &enumerations, corrections)?,
                }));
            }

            if !dump
                .classes
                .iter()
                .any(|value| value.get("Name").and_then(Value::as_str) == Some(&entry.name))
                && !declarations
                    .iter()
                    .any(|value| value.is_type && value.name == entry.name)
            {
                result
                    .data_types
                    .push(json!({"Name": entry.name, "Members": members}));
            }

            if !constructors.is_empty()
                && !declarations
                    .iter()
                    .any(|value| !value.is_type && value.name == entry.name)
            {
                result
                    .constructors
                    .push(json!({"Name": entry.name, "Members": constructors}));
            }
        } else if path.starts_with("libraries/") {
            if declarations
                .iter()
                .chain(imported)
                .any(|value| !value.is_type && value.name == entry.name)
                || (builtins
                    .iter()
                    .any(|value| !value.is_type && !value.is_member && value.name == entry.name)
                    && !result.replacements.contains_key(&entry.name))
            {
                continue;
            }

            let mut members = Vec::new();

            for group in ["properties", "functions"] {
                for member in children(entry, group)? {
                    if !corrections.exclusions.globals.contains(&member.name) {
                        members.push(convert(
                            &member,
                            group,
                            &entry.name,
                            &enumerations,
                            corrections,
                        )?);
                    }
                }
            }

            let prefix = format!("{}.", entry.name);

            for builtin in builtins
                .iter()
                .filter(|value| !value.is_type && value.is_member)
            {
                if let Some(name) = builtin.name.strip_prefix(&prefix)
                    && !members
                        .iter()
                        .any(|value| value.get("Name").and_then(Value::as_str) == Some(name))
                {
                    members.push(json!({"Name": name, "MemberType": "Property", "ValueType": {"Name": format!("typeof({})", builtin.name)}}));
                }
            }

            result
                .constructors
                .push(json!({"Name": entry.name, "Members": members}));
        } else if path == "globals/RobloxGlobals.yaml" {
            for group in ["properties", "functions"] {
                for member in children(entry, group)? {
                    if matches!(member.name.as_str(), "Enum" | "shared")
                        || corrections.exclusions.globals.contains(&member.name)
                        || declarations
                            .iter()
                            .any(|value| !value.is_type && value.name == member.name)
                        || builtins.iter().any(|value| {
                            !value.is_type && !value.is_member && value.name == member.name
                        })
                    {
                        continue;
                    }

                    let mut value = convert(&member, group, "", &enumerations, corrections)?;

                    if group == "functions" {
                        value["MemberType"] = Value::String("Callback".to_owned());
                    }

                    result.globals.push(value);
                }
            }
        }
    }

    Ok(result)
}

fn children(entry: &Entry, name: &str) -> Result<Vec<Entry>, Error> {
    entry
        .fields
        .get(name)
        .filter(|value| !value.is_null())
        .map_or_else(
            || Ok(Vec::new()),
            |value| {
                mapping::decode(
                    value.clone(),
                    &format!("Creator Docs {} {name}", entry.name),
                )
            },
        )
}

fn field<'entry>(entry: &'entry Entry, name: &str) -> Result<&'entry str, Error> {
    entry
        .fields
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| {
            invalid(format!(
                "Missing Creator Docs `{name}` for `{}`.",
                entry.name
            ))
        })
}

fn convert(
    entry: &Entry,
    group: &str,
    owner: &str,
    enumerations: &BTreeSet<&str>,
    corrections: &Corrections,
) -> Result<Value, Error> {
    let name = entry
        .name
        .rsplit(['.', ':'])
        .next()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("Unnamed Creator Docs member."))?;

    let mut value = json!({"Name": name, "Tags": entry.fields.get("tags")});

    if matches!(group, "properties" | "constants") {
        value["MemberType"] = json!("Property");
        value["ValueType"] = value_type(field(entry, "type")?, enumerations, corrections)?;
    } else {
        value["MemberType"] = json!("Function");

        let parameters = entry.parameters.as_ref().ok_or_else(|| {
            invalid(format!(
                "Missing Creator Docs parameters for `{}`.",
                entry.name
            ))
        })?;

        let mut converted = Vec::new();

        for parameter in parameters {
            let mut value = json!({"Name": parameter.name, "Type": value_type(field(parameter, "type")?, enumerations, corrections)?});

            if let Some(default) = parameter
                .fields
                .get("default")
                .filter(|value| !value.is_null())
            {
                value["Default"] = default.clone();
            }

            converted.push(value);
        }

        value["Parameters"] = Value::Array(converted);

        value["TupleReturns"] = if let Some(returns) = &entry.returns {
            Value::Array(
                returns
                    .iter()
                    .filter(|value| value.fields.get("type").and_then(Value::as_str) != Some("()"))
                    .map(|value| value_type(field(value, "type")?, enumerations, corrections))
                    .collect::<Result<_, _>>()?,
            )
        } else if group == "constructors" {
            json!([{"Name": owner}])
        } else {
            return Err(invalid(format!(
                "Missing Creator Docs returns for `{}`.",
                entry.name
            )));
        };
    }

    Ok(value)
}

fn value_type(
    name: &str,
    enumerations: &BTreeSet<&str>,
    corrections: &Corrections,
) -> Result<Value, Error> {
    let name = name.trim();

    if let Some(element) = name
        .strip_prefix("Array<")
        .and_then(|value| value.strip_suffix('>'))
    {
        let value: Type = mapping::decode(
            value_type(element, enumerations, corrections)?,
            "Creator Docs array element",
        )?;

        return Ok(json!({"Generic": value.resolve(&corrections.types)?}));
    }

    if let Some(element) = name
        .strip_prefix("Tuple<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return Ok(json!({"Tuple": value_type(element, enumerations, corrections)?}));
    }

    if name.contains('|') {
        return Ok(
            json!({"Union": name.split('|').map(|value| value_type(value, enumerations, corrections)).collect::<Result<Vec<_>, _>>()?}),
        );
    }

    if let Some(element) = name.strip_suffix('?') {
        let value: Type = mapping::decode(
            value_type(element, enumerations, corrections)?,
            "Creator Docs optional type",
        )?;

        return Ok(json!({"Name": format!("{}?", value.resolve(&corrections.types)?)}));
    }

    Ok(if enumerations.contains(name) {
        json!({"Name": name, "Category": "Enum"})
    } else {
        json!({"Name": name})
    })
}
