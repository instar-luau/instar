use std::{collections::BTreeSet, fmt::Write as _};

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use pulldown_cmark_to_cmark::cmark;
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{MarkupContent, MarkupKind},
    jsonrpc::Result,
};

use crate::{documentation, error::failure, project::Project};

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
    code: &str,
    documentation: &str,
    should_use_markdown: bool,
) -> Result<String> {
    let mut output = String::new();

    if !code.is_empty() {
        append(&mut output, &render_code(code, should_use_markdown)?);
    }

    if !documentation.is_empty() {
        if should_use_markdown && !output.is_empty() {
            append(&mut output, "---");
        }

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

pub(crate) fn completion(
    project: &Project,
    module: &str,
    value: Option<&Value>,
    hide_receiver: bool,
    should_use_markdown: bool,
) -> Result<String> {
    let mut sections = Vec::new();

    let text = render(project, module, value, hide_receiver, should_use_markdown)?;

    if !text.is_empty() {
        sections.push(text);
    }

    if let Some(overloads) = value
        .and_then(|value| value.get("overloads"))
        .and_then(Value::as_object)
    {
        let mut descriptions = BTreeSet::new();

        for (signature, reference) in overloads {
            let Some(reference) = reference.as_str() else {
                continue;
            };

            let metadata = documentation::metadata(project, module, reference)?;

            let text = render(
                project,
                module,
                metadata.as_deref(),
                hide_receiver,
                should_use_markdown,
            )?;

            let description = if descriptions.insert(text.clone()) {
                text.as_str()
            } else {
                ""
            };

            sections.push(presentation(signature, description, should_use_markdown)?);
        }
    }

    Ok(hover(&sections, should_use_markdown).value)
}

pub(crate) fn render(
    project: &Project,
    module: &str,
    value: Option<&Value>,
    hide_receiver: bool,
    should_use_markdown: bool,
) -> Result<String> {
    let referenced;

    let value = if let Some(reference) = value
        .and_then(documentation::body)
        .filter(|text| documentation::is_alias(text))
    {
        referenced = documentation::metadata(project, module, reference)?;

        referenced.as_deref()
    } else {
        value
    };

    let description = value.and_then(documentation::body).unwrap_or_default();

    let (description, link) = description
        .rsplit_once("\n\n[Learn more](")
        .map_or((description, None), |(text, link)| (text, Some(link)));

    let mut text = description.to_owned();
    let mut parameters = Vec::new();

    if let Some(entries) = value
        .and_then(|value| value.get("params"))
        .and_then(Value::as_array)
    {
        let receiver = hide_receiver
            && entries
                .first()
                .and_then(|entry| entry.get("name"))
                .and_then(Value::as_str)
                == Some("self");

        for entry in entries.iter().skip(usize::from(receiver)) {
            let Some(name) = entry.get("name").and_then(Value::as_str) else {
                continue;
            };

            let description = description_text(project, module, entry)?;

            if !description.is_empty() {
                parameters.push(format!("- `{name}` — {description}"));
            }
        }
    }

    if !parameters.is_empty() {
        append(&mut text, "### Parameters");
        append(&mut text, &parameters.join("\n"));
    }

    let mut returns = Vec::new();

    if let Some(entries) = value
        .and_then(|value| value.get("returns"))
        .and_then(Value::as_array)
    {
        for entry in entries {
            let description = description_text(project, module, entry)?;

            if !description.is_empty() && !returns.contains(&description) {
                returns.push(description);
            }
        }
    }

    if !returns.is_empty() {
        append(&mut text, "### Returns");
        append(&mut text, &returns.join("\n\n"));
    }

    if let Some(link) = link {
        append(&mut text, &format!("[Learn more]({link}"));
    }

    Ok(if should_use_markdown {
        text
    } else {
        plaintext(&text)
    })
}

fn description_text(project: &Project, module: &str, value: &Value) -> Result<String> {
    let text = documentation::body(value).unwrap_or_default();

    if documentation::is_alias(text) {
        let value = documentation::metadata(project, module, text)?;

        Ok(value
            .as_deref()
            .and_then(documentation::body)
            .unwrap_or_default()
            .to_owned())
    } else {
        Ok(text.to_owned())
    }
}

fn plaintext(text: &str) -> String {
    let mut output = String::new();
    let mut lists = Vec::new();
    let mut links = Vec::new();

    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;

    for event in Parser::new_ext(text, options) {
        match event {
            Event::Text(text) | Event::Code(text) => output.push_str(&text),
            Event::SoftBreak | Event::HardBreak => output.push('\n'),
            Event::Start(Tag::List(start)) => lists.push(start),

            Event::End(TagEnd::List(_)) => {
                lists.pop();
            }

            Event::Start(Tag::Item) => {
                newline(&mut output);
                output.push_str(&"  ".repeat(lists.len().saturating_sub(1)));

                if let Some(Some(number)) = lists.last_mut() {
                    write!(output, "{number}. ")
                        .expect("Internal error: cannot write to a string.");

                    *number = number.saturating_add(1);
                } else {
                    output.push_str("- ");
                }
            }

            Event::End(TagEnd::Item | TagEnd::TableHead | TagEnd::TableRow) => {
                newline(&mut output);
            }

            Event::End(TagEnd::TableCell) => output.push('\t'),

            Event::Start(Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. }) => {
                links.push((output.len(), dest_url));
            }

            Event::End(TagEnd::Link | TagEnd::Image) => {
                if let Some((start, destination)) = links.pop() {
                    let label = &output[start..];

                    if label.is_empty() {
                        output.push_str(&destination);
                    } else if label != destination.as_ref() {
                        output.push_str(" (");
                        output.push_str(&destination);
                        output.push(')');
                    }
                }
            }

            Event::TaskListMarker(checked) => {
                output.push_str(if checked { "[x] " } else { "[ ] " });
            }

            Event::End(
                TagEnd::Paragraph
                | TagEnd::Heading(_)
                | TagEnd::BlockQuote(_)
                | TagEnd::CodeBlock
                | TagEnd::Table,
            )
            | Event::Rule => {
                newline(&mut output);

                if !output.ends_with("\n\n") {
                    output.push('\n');
                }
            }

            _ => {}
        }
    }

    output.trim_end_matches('\n').to_owned()
}

fn newline(output: &mut String) {
    while output.ends_with('\t') {
        output.pop();
    }

    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn render_code(text: &str, should_use_markdown: bool) -> Result<String> {
    if !should_use_markdown {
        return Ok(text.to_owned());
    }

    let text = if text.ends_with('\n') {
        text.to_owned()
    } else {
        format!("{text}\n")
    };

    let events = [
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced("luau".into()))),
        Event::Text(text.into()),
        Event::End(TagEnd::CodeBlock),
    ];

    let mut output = String::new();
    cmark(&events, &mut output, Options::empty()).map_err(failure)?;

    Ok(output.trim().to_owned())
}

fn append(text: &mut String, section: &str) {
    if !text.is_empty() {
        text.push_str("\n\n");
    }

    text.push_str(section);
}
