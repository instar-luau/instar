use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
};

use htmd::{Element, HtmlToMarkdown, element_handler::Handlers};
use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd};
use pulldown_cmark_to_cmark::cmark;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Error, mapping::invalid};

#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
struct Document {
    #[serde(skip_serializing_if = "String::is_empty")]
    documentation: String,

    #[serde(rename = "params", skip_serializing_if = "Vec::is_empty")]
    parameters: Vec<Parameter>,

    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    overloads: BTreeMap<String, String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    returns: Vec<String>,

    #[serde(rename = "code_sample", skip_serializing)]
    sample: String,

    #[serde(rename = "learn_more_link", skip_serializing)]
    link: String,

    #[serde(skip_serializing)]
    base: String,
}

#[derive(Deserialize, Serialize)]
struct Parameter {
    name: String,

    #[serde(rename = "documentation")]
    reference: String,
}

pub(super) fn prepare(value: &Value, documents: &Map<String, Value>) -> Result<Value, Error> {
    let mut document = parse(value)?;

    if is_alias(&document.documentation, documents) {
        resolve(&document.documentation, documents)?;

        return Ok(json!({ "documentation": document.documentation }));
    }

    let base = Url::parse(if document.base.is_empty() {
        &document.link
    } else {
        &document.base
    })
    .ok();

    let mut markdown = convert(&document.documentation, base.as_ref())?;

    for reference in &document.returns {
        resolve(reference, documents)?;
    }

    if !document.sample.is_empty() {
        let events = code(mem::take(&mut document.sample), "luau");
        append(&mut markdown, &serialize(&events)?);
    }

    if let Some(address) = Url::parse(&document.link)
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))
    {
        let tag = Tag::Link {
            link_type: LinkType::Inline,
            dest_url: address.as_str().into(),
            title: "".into(),
            id: "".into(),
        };

        let events = [
            Event::Start(tag.clone()),
            Event::Text("Learn more".into()),
            Event::End(tag.to_end()),
        ];

        append(&mut markdown, &serialize(&events)?);
    }

    document.documentation = markdown;

    serde_json::to_value(document).map_err(|source| Error::Data {
        input: "documentation".to_owned(),
        source,
    })
}

fn parse(value: &Value) -> Result<Document, Error> {
    if let Value::String(text) = value {
        return Ok(Document {
            documentation: text.clone(),
            ..Document::default()
        });
    }

    serde_json::from_value(value.clone()).map_err(|source| Error::Data {
        input: "documentation".to_owned(),
        source,
    })
}

fn resolve<'documents>(
    mut reference: &'documents str,
    documents: &'documents Map<String, Value>,
) -> Result<&'documents Value, Error> {
    let mut visited = BTreeSet::new();

    while visited.insert(reference) {
        let value = documents
            .get(reference)
            .ok_or_else(|| invalid(format!("Missing documentation reference `{reference}`.")))?;

        let text = value
            .as_str()
            .or_else(|| value.get("documentation").and_then(Value::as_str))
            .unwrap_or_default();

        if !is_alias(text, documents) {
            return Ok(value);
        }

        reference = text;
    }

    Err(invalid(format!(
        "Cyclic documentation reference `{reference}`."
    )))
}

fn is_alias(text: &str, documents: &Map<String, Value>) -> bool {
    documents.contains_key(text)
        || (text.starts_with('@') && text.contains('/') && !text.contains(char::is_whitespace))
}

fn convert(text: &str, base: Option<&Url>) -> Result<String, Error> {
    let converter = HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style"])
        .add_handler(
            vec!["sup"],
            |handlers: &dyn Handlers, element: Element<'_>| {
                Some(format!("^({})", handlers.walk_children(element.node).content).into())
            },
        )
        .build();

    let markdown = converter
        .convert(text)
        .map_err(|error| invalid(format!("Could not convert documentation. {error}")))?;

    let mut links = Vec::new();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;

    let events: Vec<_> = Parser::new_ext(&markdown, options)
        .filter_map(|event| match event {
            Event::Start(mut tag) => {
                if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } = &mut tag {
                    if let Some(address) = destination(dest_url, base) {
                        *dest_url = address.to_string().into();
                        links.push(None);
                    } else {
                        links.push(Some(format!(" ({dest_url})")));

                        return None;
                    }
                }

                Some(Event::Start(tag))
            }

            Event::End(TagEnd::Link | TagEnd::Image) => {
                match links
                    .pop()
                    .expect("Internal error: unmatched documentation link.")
                {
                    Some(address) => Some(Event::Text(address.into())),
                    None => Some(event),
                }
            }

            Event::Html(_) | Event::InlineHtml(_) => None,
            _ => Some(event),
        })
        .collect();

    serialize(&events)
}

fn destination(address: &str, base: Option<&Url>) -> Option<Url> {
    if address.is_empty() {
        return None;
    }

    Url::parse(address)
        .ok()
        .or_else(|| base?.join(address).ok())
        .filter(|url| matches!(url.scheme(), "http" | "https"))
}

fn code(mut text: String, language: &str) -> Vec<Event<'static>> {
    if !text.ends_with('\n') {
        text.push('\n');
    }

    vec![
        Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
            language.to_owned().into(),
        ))),
        Event::Text(text.into()),
        Event::End(TagEnd::CodeBlock),
    ]
}

fn serialize(events: &[Event<'_>]) -> Result<String, Error> {
    let mut output = String::new();
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;

    cmark(events, &mut output, options)
        .map_err(|error| invalid(format!("Could not serialize documentation. {error}")))?;

    let breaks: Vec<_> = Parser::new_ext(&output, options)
        .into_offset_iter()
        .filter_map(|(event, range)| matches!(event, Event::HardBreak).then_some(range))
        .collect();

    for range in breaks.into_iter().rev() {
        output.replace_range(range, "\\\n");
    }

    Ok(output.trim().to_owned())
}

fn append(output: &mut String, text: &str) {
    if !text.is_empty() {
        if !output.is_empty() {
            output.push_str("\n\n");
        }

        output.push_str(text);
    }
}
