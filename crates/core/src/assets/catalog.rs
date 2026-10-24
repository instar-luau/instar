use std::collections::{BTreeMap, BTreeSet};

use instar_bridge::Builtin;
use serde_json::{Map, Value, json};

use super::{
    Error,
    generation::interrupted,
    mapping::{self, invalid},
    reference, syntax,
};

pub(super) struct Document {
    pub(super) documentation: String,
    pub(super) link: String,
    pub(super) parameters: Option<Vec<Parameter>>,
    pub(super) returns: Option<Vec<String>>,
    pub(super) has_receiver: bool,
}

pub(super) struct Parameter {
    pub(super) name: String,
    pub(super) documentation: String,
}

pub(super) fn assemble(
    documentation: Value,
    creator: BTreeMap<String, Vec<Document>>,
    references: reference::Catalog,
    builtins: &[Builtin],
    definitions: &BTreeMap<String, String>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Map<String, Value>, Error> {
    let mut roblox = BTreeSet::new();

    for definition in definitions.values() {
        for block in syntax::blocks(definition, is_cancelled)? {
            roblox.insert((block.is_type, block.name));
        }
    }

    let mut replacements = BTreeMap::new();

    for builtin in builtins.iter().filter(|builtin| !builtin.is_member) {
        interrupted(is_cancelled)?;

        if !roblox.contains(&(builtin.is_type, builtin.name.clone()))
            && let Some(symbol) = builtin.documentation.strip_prefix("@luau/")
        {
            replacements.insert(format!("@roblox/{symbol}"), builtin.documentation.clone());
        }
    }

    let mut upstream: Map<String, Value> = mapping::decode(documentation, "documentation")?;
    merge(&mut upstream, creator, is_cancelled)?;
    let upstream = canonical(Value::Object(upstream), &replacements, is_cancelled)?;
    let upstream: Map<String, Value> = mapping::decode(upstream, "documentation")?;

    let required: BTreeSet<_> = builtins
        .iter()
        .map(|builtin| &builtin.documentation)
        .filter(|symbol| !symbol.is_empty())
        .cloned()
        .collect();

    for symbol in references
        .documents
        .keys()
        .chain(references.additions.keys())
        .chain(references.corrections.keys())
    {
        interrupted(is_cancelled)?;

        if !required.contains(symbol) && !upstream.contains_key(symbol) {
            return Err(invalid(format!("Unknown documentation symbol `{symbol}`.")));
        }
    }

    let mut documents = references.documents;
    overlay(&mut documents, upstream, is_cancelled)?;

    for (symbol, value) in references.additions {
        interrupted(is_cancelled)?;
        documents.entry(symbol).or_insert(value);
    }

    overlay(&mut documents, references.corrections, is_cancelled)?;

    let missing: Vec<_> = required
        .into_iter()
        .filter(|symbol| !documents.contains_key(symbol))
        .collect();

    if !missing.is_empty() {
        return Err(invalid(format!(
            "Missing built-in documentation: {}.",
            missing.join(", ")
        )));
    }

    Ok(documents)
}

fn canonical(
    value: Value,
    replacements: &BTreeMap<String, String>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Value, Error> {
    interrupted(is_cancelled)?;

    match value {
        Value::Object(values) => {
            let keys: BTreeSet<_> = values.keys().cloned().collect();
            let mut result = Map::new();

            for (key, value) in values {
                interrupted(is_cancelled)?;
                let replacement = canonical_symbol(&key, replacements);

                if replacement != key && keys.contains(&replacement) {
                    continue;
                }

                result.insert(replacement, canonical(value, replacements, is_cancelled)?);
            }

            Ok(Value::Object(result))
        }

        Value::Array(values) => {
            let values: Result<Vec<_>, _> = values
                .into_iter()
                .map(|value| canonical(value, replacements, is_cancelled))
                .collect();

            Ok(Value::Array(values?))
        }

        Value::String(value) => Ok(Value::String(canonical_symbol(&value, replacements))),
        value => Ok(value),
    }
}

fn canonical_symbol(value: &str, replacements: &BTreeMap<String, String>) -> String {
    let mut root = value;

    loop {
        if let Some(replacement) = replacements.get(root) {
            return format!("{replacement}{}", &value[root.len()..]);
        }

        let Some(boundary) = root.rfind(['.', '/']) else {
            return value.to_owned();
        };

        root = &root[..boundary];
    }
}

fn merge(
    documents: &mut Map<String, Value>,
    incoming: BTreeMap<String, Vec<Document>>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    for (symbol, entries) in incoming {
        interrupted(is_cancelled)?;

        let overloads: Vec<String> = documents
            .get(&symbol)
            .and_then(|value| value.get("overloads"))
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|values| values.values())
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();

        for entry in entries {
            interrupted(is_cancelled)?;

            if overloads.is_empty() {
                enrich(documents, &symbol, &entry)?;
            }

            for overload in &overloads {
                let parameters = documents
                    .get(overload)
                    .and_then(|value| value.get("params"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|value| value.get("name").and_then(Value::as_str))
                    .filter(|name| *name != "self");

                if parameters.eq(entry
                    .parameters
                    .iter()
                    .flatten()
                    .map(|value| value.name.as_str()))
                {
                    enrich(documents, overload, &entry)?;
                }
            }
        }
    }

    Ok(())
}

fn enrich(documents: &mut Map<String, Value>, symbol: &str, entry: &Document) -> Result<(), Error> {
    let mut value = documents.remove(symbol).unwrap_or_else(|| json!({}));

    if value.is_string() {
        value = json!({ "documentation": value });
    }

    let document = value
        .as_object_mut()
        .ok_or_else(|| invalid(format!("Invalid documentation `{symbol}`.")))?;

    if !entry.documentation.is_empty() {
        document.insert(
            "documentation".into(),
            Value::String(entry.documentation.clone()),
        );

        document.insert("learn_more_link".into(), Value::String(entry.link.clone()));
    }

    if let Some(parameters) = &entry.parameters {
        let mut references = document
            .get("params")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        if references.is_empty() {
            if entry.has_receiver {
                let reference = format!("{symbol}/param/0");
                documents.insert(reference.clone(), json!({"documentation": ""}));
                references.push(json!({"name": "self", "documentation": reference}));
            }

            for parameter in parameters {
                let reference = format!("{symbol}/param/{}", references.len());
                references.push(json!({"name": parameter.name, "documentation": reference}));
            }
        }

        for parameter in parameters {
            for reference in &references {
                if reference.get("name").and_then(Value::as_str) == Some(&parameter.name)
                    && let Some(symbol) = reference.get("documentation").and_then(Value::as_str)
                {
                    documents.insert(
                        symbol.to_owned(),
                        json!({"documentation": parameter.documentation, "base": entry.link}),
                    );
                }
            }
        }

        document.insert("params".into(), Value::Array(references));
    }

    if let Some(returns) = &entry.returns {
        let mut references = Vec::new();

        for (index, returned) in returns.iter().enumerate() {
            let reference = format!("{symbol}/return/{index}");

            documents.insert(
                reference.clone(),
                json!({"documentation": returned, "base": entry.link}),
            );

            references.push(Value::String(reference));
        }

        document.insert("returns".into(), Value::Array(references));
    }

    documents.insert(symbol.to_owned(), value);

    Ok(())
}

fn overlay(
    documents: &mut Map<String, Value>,
    patches: Map<String, Value>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    for (symbol, patch) in patches {
        interrupted(is_cancelled)?;

        match (documents.get_mut(&symbol), patch) {
            (Some(Value::Object(document)), Value::Object(patch)) => document.extend(patch),

            (_, patch) => {
                documents.insert(symbol, patch);
            }
        }
    }

    Ok(())
}
