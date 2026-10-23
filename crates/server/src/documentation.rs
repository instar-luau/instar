use std::{collections::BTreeSet, sync::Arc};

use instar_bridge::Documentation;

use serde_json::Value;
use tower_lsp_server::jsonrpc::Result;

use crate::{error::failure, project::Project};

pub(crate) fn resolve(
    project: &mut Project,
    module: &str,
    reference: &Documentation,
) -> Result<Option<Arc<Value>>> {
    let symbol = if reference.signature.is_empty() {
        reference.symbol.as_str()
    } else {
        reference
            .symbol
            .split_once("/overload/")
            .map_or(reference.symbol.as_str(), |(root, _)| root)
    };

    let mut value = metadata(project, module, symbol)?;

    if !reference.signature.is_empty()
        && let Some(symbol) = value
            .as_deref()
            .and_then(|value| value.get("overloads"))
            .and_then(|overloads| overloads.get(&reference.signature))
            .and_then(Value::as_str)
    {
        value = metadata(project, module, symbol)?;
    }

    if value.is_some() {
        return Ok(value);
    }

    for declaration in &reference.declarations {
        let document = project.snapshot(&declaration.module)?;
        let range = document.native_bytes(declaration.range)?;

        if let Some(value) = document.analysis().documentation(range.start) {
            return Ok(Some(Arc::new(value)));
        }
    }

    Ok(value)
}

pub(crate) fn metadata(
    project: &Project,
    module: &str,
    symbol: &str,
) -> Result<Option<Arc<Value>>> {
    let mut symbol = symbol.to_owned();
    let mut visited = BTreeSet::new();

    loop {
        if symbol.is_empty() || !visited.insert(symbol.clone()) {
            return Ok(None);
        }

        let value = project
            .workspace
            .documentation(module, &symbol)
            .map_err(failure)?;

        if let Some(reference) = value
            .as_deref()
            .and_then(body)
            .filter(|text| is_alias(text))
        {
            reference.clone_into(&mut symbol);
        } else {
            return Ok(value);
        }
    }
}

pub(crate) fn parameter<'metadata>(
    metadata: Option<&'metadata Value>,
    name: &str,
    source_index: u32,
) -> Option<&'metadata Value> {
    let parameters = metadata?.get("params")?.as_array()?;

    parameters
        .iter()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .or_else(|| {
            usize::try_from(source_index)
                .ok()
                .and_then(|index| parameters.get(index))
        })
}

pub(crate) fn body(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("documentation").and_then(Value::as_str))
}

pub(crate) fn is_alias(text: &str) -> bool {
    text.starts_with('@') && text.contains('/') && !text.contains(['\n', '\r'])
}
