use std::collections::BTreeSet;

use instar_core::presentation::Section;

use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{MarkupContent, MarkupKind, Uri},
    jsonrpc::Result,
};

use crate::{error::failure, output::content, project::Project};

pub(crate) fn should_use_markdown(project: &Project, pointer: &str) -> bool {
    project
        .capabilities
        .pointer(pointer)
        .and_then(Value::as_array)
        .and_then(|formats| {
            formats.iter().find_map(|format| match format.as_str() {
                Some("markdown") => Some(true),
                Some("plaintext") => Some(false),
                _ => None,
            })
        })
        .unwrap_or(false)
}

pub(crate) fn presentation(
    sections: &[Section],
    documentation: &str,
    should_use_markdown: bool,
) -> Result<String> {
    let mut output = String::new();

    if let Some(primary) = sections.first() {
        append(
            &mut output,
            &content::code(&primary.content, should_use_markdown)?,
        );
    }

    if !documentation.is_empty() {
        append(&mut output, documentation);
    }

    Ok(output)
}

pub(crate) fn hover(sections: &[String], should_use_markdown: bool) -> MarkupContent {
    markup(
        sections.join(if should_use_markdown {
            "\n\n---\n\n"
        } else {
            "\n\n"
        }),
        should_use_markdown,
    )
}

pub(crate) fn markup(value: String, should_use_markdown: bool) -> MarkupContent {
    MarkupContent {
        kind: if should_use_markdown {
            MarkupKind::Markdown
        } else {
            MarkupKind::PlainText
        },
        value,
    }
}

pub(crate) fn documentation(
    project: &Project,
    module: &str,
    symbol: &str,
    fallback: &str,
    should_use_markdown: bool,
) -> Result<String> {
    let value = metadata(project, module, symbol)?;

    render(
        project,
        module,
        value.as_ref(),
        fallback,
        should_use_markdown,
    )
}

pub(crate) fn metadata(project: &Project, module: &str, symbol: &str) -> Result<Option<Value>> {
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

        if let Some(reference) = value.as_ref().and_then(body).filter(|text| is_alias(text)) {
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

pub(crate) fn render(
    project: &Project,
    module: &str,
    value: Option<&Value>,
    fallback: &str,
    should_use_markdown: bool,
) -> Result<String> {
    let referenced;

    let value = if let Some(reference) = value.and_then(body).filter(|text| is_alias(text)) {
        referenced = metadata(project, module, reference)?;

        referenced.as_ref()
    } else {
        value
    };

    let mut text = content::render(
        value.and_then(body).unwrap_or(fallback),
        should_use_markdown,
    )?;

    if let Some(sample) = value
        .and_then(|value| value.get("code_sample"))
        .and_then(Value::as_str)
        .filter(|sample| !sample.is_empty())
    {
        append(&mut text, &content::code(sample, should_use_markdown)?);
    }

    if let Some(link) = value
        .and_then(|value| value.get("learn_more_link"))
        .and_then(Value::as_str)
        .filter(|link| {
            link.parse::<Uri>()
                .is_ok_and(|uri| matches!(uri.scheme(), "http" | "https"))
        })
    {
        append(
            &mut text,
            &content::link("Learn more", link, should_use_markdown)?,
        );
    }

    Ok(text)
}

fn body(value: &Value) -> Option<&str> {
    value
        .as_str()
        .or_else(|| value.get("documentation").and_then(Value::as_str))
}

fn is_alias(text: &str) -> bool {
    text.starts_with('@') && text.contains('/') && !text.contains(char::is_whitespace)
}

fn append(text: &mut String, section: &str) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }

    text.push_str(section);
}
