use crate::{
    analysis::{EditorResult, Session},
    source::{PositionEncoding, Source, SourceStore},
};
use serde_json::Value;
use std::{io, ops::Range, path::Path};

pub(super) fn parse(
    session: &mut Session,
    sources: &mut SourceStore,
    path: &Path,
) -> io::Result<Value> {
    let report = session.query(
        sources,
        &[path.to_owned()],
        path,
        line_index::LineCol { line: 0, col: 0 },
        "syntax",
    )?;

    if let Some(error) = report.diagnostics.iter().find(|diagnostic| {
        diagnostic.path == path && diagnostic.message.starts_with("SyntaxError:")
    }) {
        return Err(io::Error::other(format!(
            "{}:{}:{}: {}",
            path.display(),
            error.line + 1,
            error.column + 1,
            error.message
        )));
    }

    let Some(EditorResult::Entry(entry)) = report.editor else {
        return Err(io::Error::other("build syntax unavailable"));
    };

    let mut document: Value = serde_json::from_str(
        entry
            .description
            .as_deref()
            .ok_or_else(|| io::Error::other("build syntax unavailable"))?,
    )
    .map_err(io::Error::other)?;

    let links = session.query(
        sources,
        &[path.to_owned()],
        path,
        line_index::LineCol { line: 0, col: 0 },
        "links",
    )?;

    if let Some(EditorResult::Entries(entries)) = links.editor {
        document["dependencies"] = Value::Array(
            entries
                .into_iter()
                .filter(|entry| entry.path.as_deref() == Some(path))
                .filter_map(|entry| {
                    Some(serde_json::json!({"path":entry.name?,"range":entry.range?}))
                })
                .collect(),
        );
    }

    Ok(document)
}

pub(super) fn nodes(value: &Value) -> Vec<&Value> {
    let mut result = Vec::new();
    let mut pending = vec![value];

    while let Some(value) = pending.pop() {
        match value {
            Value::Object(fields) => {
                if value["type"].as_str().is_some() {
                    result.push(value);
                }

                pending.extend(
                    fields
                        .iter()
                        .filter(|(name, _)| {
                            name.as_str() != "local" && name.as_str() != "prefixLocal"
                        })
                        .map(|(_, value)| value),
                );
            }

            Value::Array(values) => pending.extend(values),
            _ => {}
        }
    }

    result
}

pub(super) fn kind(value: &Value) -> &str {
    value["type"].as_str().unwrap_or_default()
}

pub(super) fn field<'value>(value: &'value Value, name: &str) -> &'value str {
    value[name].as_str().unwrap_or_default()
}

pub(super) fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

pub(super) fn unwrap(mut value: &Value) -> &Value {
    while matches!(kind(value), "AstExprGroup" | "AstExprTypeAssertion") {
        value = &value["expr"];
    }

    value
}

pub(super) fn range(source: &Source, value: &Value) -> io::Result<Range<usize>> {
    let (start, end) = field(value, "location")
        .split_once(" - ")
        .ok_or_else(|| io::Error::other("missing syntax location"))?;

    let position = |value: &str| -> io::Result<usize> {
        let (line, column) = value
            .split_once(',')
            .ok_or_else(|| io::Error::other("invalid syntax location"))?;

        Ok(usize::from(
            source
                .offset(
                    line_index::LineCol {
                        line: line.parse().map_err(io::Error::other)?,
                        col: column.parse().map_err(io::Error::other)?,
                    },
                    PositionEncoding::Utf8,
                )
                .map_err(io::Error::other)?,
        ))
    };

    Ok(position(start)?..position(end)?)
}

pub(super) fn quote(value: &str) -> String {
    use std::fmt::Write;
    let mut result = String::from("\"");

    for byte in value.bytes() {
        match byte {
            b'"' => result.push_str("\\\""),
            b'\\' => result.push_str("\\\\"),
            b' '..=b'~' => result.push(char::from(byte)),
            _ => write!(result, "\\{byte:03}").expect("string write"),
        }
    }

    result.push('"');

    result
}
