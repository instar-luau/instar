use std::{collections::BTreeMap, ops::Range};

use instar_bridge::Builtin;
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd, html};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::{
    Error,
    generation::download,
    mapping::{self, invalid},
};

use crate::syntax::is_identifier;

#[derive(Default)]
pub(super) struct Catalog {
    pub(super) documents: Map<String, Value>,
    pub(super) additions: Map<String, Value>,
    pub(super) corrections: Map<String, Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase", deny_unknown_fields)]
struct Configuration {
    sources: BTreeMap<String, String>,

    #[serde(default)]
    symbols: BTreeMap<String, String>,

    #[serde(default)]
    imports: BTreeMap<String, String>,

    #[serde(default)]
    additions: Map<String, Value>,

    #[serde(default)]
    corrections: Map<String, Value>,
}

struct Section {
    range: Range<usize>,
    symbols: Vec<String>,
    link: String,
}

pub(super) async fn acquire(client: &Client, builtins: &[Builtin]) -> Result<Catalog, Error> {
    let configuration: Configuration = mapping::parse(
        include_str!("../../assets/documentation.json"),
        "documentation configuration",
    )?;

    let mut documents = Map::new();

    let symbols: BTreeMap<_, _> = builtins
        .iter()
        .filter(|builtin| {
            (!builtin.is_type || builtin.is_member) && !builtin.documentation.is_empty()
        })
        .map(|builtin| (builtin.name.clone(), builtin.documentation.clone()))
        .collect();

    for (link, address) in &configuration.sources {
        let contents = download(client, address).await?;

        let source = String::from_utf8(contents).map_err(|error| {
            invalid(format!(
                "Documentation source `{address}` is not valid UTF-8. {error}"
            ))
        })?;

        documents.extend(parse(&source, link, &configuration.symbols, &symbols)?);
    }

    let imports: Result<Map<_, _>, Error> = configuration
        .imports
        .into_iter()
        .map(|(symbol, reference)| {
            let value = documents.get(&reference).cloned().ok_or_else(|| {
                invalid(format!(
                    "Missing documentation import `{reference}` for `{symbol}`."
                ))
            })?;

            Ok((symbol, value))
        })
        .collect();

    documents.extend(imports?);

    documents.retain(|symbol, _| symbol.starts_with('@'));

    Ok(Catalog {
        documents,
        additions: configuration.additions,
        corrections: configuration.corrections,
    })
}

fn parse(
    source: &str,
    link: &str,
    aliases: &BTreeMap<String, String>,
    symbols: &BTreeMap<String, String>,
) -> Result<Map<String, Value>, Error> {
    let source = if let Some(source) = source.strip_prefix("---\n") {
        source
            .split_once("\n---\n")
            .ok_or_else(|| invalid(format!("Unclosed documentation frontmatter in `{link}`.")))?
            .1
    } else {
        source
    };

    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut events = Parser::new_ext(source, options).into_offset_iter();

    let mut sections = vec![Section {
        range: 0..0,
        symbols: vec![link.to_owned()],
        link: link.to_owned(),
    }];

    let mut heading = link.to_owned();
    let mut paragraph = None;

    while let Some((event, range)) = events.next() {
        match event {
            Event::Start(Tag::Heading { .. }) => {
                let mut title = String::new();
                let mut end = range.end;

                for (event, range) in events.by_ref() {
                    match event {
                        Event::Text(text) | Event::Code(text) => title.push_str(&text),

                        Event::End(TagEnd::Heading(_)) => {
                            end = range.end;
                            break;
                        }

                        _ => {}
                    }
                }

                let slug: String = title
                    .chars()
                    .filter(|character| {
                        character.is_alphanumeric()
                            || character.is_whitespace()
                            || *character == '-'
                    })
                    .collect();

                let slug = slug
                    .to_lowercase()
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join("-");

                heading = format!("{link}#{slug}");

                sections.push(Section {
                    range: range.start..end,
                    symbols: vec![heading.clone()],
                    link: heading.clone(),
                });
            }

            Event::Start(Tag::CodeBlock(kind)) => {
                let mut code = String::new();
                let mut end = range.end;

                for (event, range) in events.by_ref() {
                    match event {
                        Event::Text(text) => code.push_str(&text),

                        Event::End(TagEnd::CodeBlock) => {
                            end = range.end;
                            break;
                        }

                        _ => {}
                    }
                }

                if matches!(kind, CodeBlockKind::Fenced(language) if language.is_empty()) {
                    let symbols: Option<Vec<_>> = code
                        .lines()
                        .filter(|line| !line.trim().is_empty())
                        .map(|line| symbol(line, aliases))
                        .collect();

                    if let Some(symbols) = symbols.filter(|symbols| !symbols.is_empty()) {
                        sections.push(Section {
                            range: range.start..end,
                            symbols,
                            link: heading.clone(),
                        });
                    }
                }
            }

            Event::Start(Tag::Paragraph) => paragraph = Some(range),
            Event::End(TagEnd::Paragraph) => paragraph = None,

            Event::Code(code) => {
                if let Some(range) = &paragraph
                    && source[range.clone()].trim() == format!("`{code}`")
                    && let Some(symbol) = symbol(&code, aliases)
                {
                    sections.push(Section {
                        range: range.clone(),
                        symbols: vec![symbol],
                        link: heading.clone(),
                    });
                }
            }

            _ => {}
        }
    }

    let mut documents: Map<String, Value> = Map::new();

    for (index, section) in sections.iter().enumerate() {
        let end = sections
            .get(index + 1)
            .map_or(source.len(), |section| section.range.start);

        let description = source[section.range.end..end].trim();

        if description.is_empty() {
            continue;
        }

        let mut body = String::new();
        html::push_html(&mut body, Parser::new_ext(description, options));

        for symbol in &section.symbols {
            let symbol = symbols.get(symbol).unwrap_or(symbol);
            let value = json!({ "documentation": body, "learn_more_link": section.link });

            if let Some(previous) = documents.get_mut(symbol) {
                let text = previous["documentation"].as_str().ok_or_else(|| {
                    invalid(format!("Invalid imported documentation `{symbol}`."))
                })?;

                if text != body {
                    previous["documentation"] = Value::String(format!("{text}\n{body}"));
                }
            } else {
                documents.insert(symbol.clone(), value);
            }
        }
    }

    Ok(documents)
}

fn symbol(signature: &str, aliases: &BTreeMap<String, String>) -> Option<String> {
    let signature = signature.trim();
    let signature = signature.strip_prefix("function ").unwrap_or(signature);

    let boundary = signature
        .find(|character: char| {
            !character.is_ascii_alphanumeric() && !matches!(character, '_' | '.' | ':')
        })
        .unwrap_or(signature.len());

    let name = signature[..boundary].trim_end_matches(':');
    let suffix = &signature[name.len()..];

    if !name.split(['.', ':']).all(is_identifier)
        || !(suffix.starts_with(['(', '<', ':']) || (suffix.is_empty() && name.contains('.')))
    {
        return None;
    }

    let name = name.replace(':', ".");

    for (prefix, replacement) in aliases {
        if name == *prefix {
            return Some(replacement.clone());
        }

        if prefix.ends_with('.')
            && let Some(suffix) = name.strip_prefix(prefix)
        {
            return Some(format!("{replacement}{suffix}"));
        }
    }

    Some(name)
}
