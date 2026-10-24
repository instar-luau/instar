use std::collections::BTreeMap;

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd, html};
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinSet;

use super::{
    Error,
    catalog::{Document, Parameter},
    generation::download,
    mapping::invalid,
};

const ROOT: &str = "content/en-us/reference/engine/";

#[derive(Deserialize)]
struct Tree {
    sha: String,
    truncated: bool,
    tree: Vec<File>,
}

#[derive(Deserialize)]
struct File {
    path: String,
}

#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    name: String,

    description: Option<String>,
    summary: Option<String>,
    parameters: Option<Vec<Self>>,
    returns: Option<Vec<Self>>,

    #[serde(flatten)]
    fields: BTreeMap<String, Value>,
}

impl Entry {
    fn prose(&self) -> &str {
        self.description
            .as_deref()
            .filter(|text| !text.trim().is_empty())
            .or(self.summary.as_deref())
            .unwrap_or_default()
            .trim()
    }
}

pub(super) async fn acquire(client: &Client) -> Result<BTreeMap<String, Vec<Document>>, Error> {
    let address = "https://api.github.com/repos/Roblox/creator-docs/git/trees/main?recursive=1";

    let failure = |source| Error::Download {
        address: address.to_owned(),
        source,
    };

    let response = client
        .get(address)
        .header(reqwest::header::USER_AGENT, "instar")
        .send()
        .await
        .map_err(failure)?
        .error_for_status()
        .map_err(failure)?;

    let contents = response.bytes().await.map_err(failure)?;

    let tree: Tree = serde_json::from_slice(&contents)
        .map_err(|error| invalid(format!("Invalid Creator Docs tree. {error}")))?;

    if tree.truncated {
        return Err(invalid("Creator Docs tree is truncated."));
    }

    let mut pending = JoinSet::new();
    let mut sources = BTreeMap::new();

    for file in tree.tree {
        let Some(path) = file.path.strip_prefix(ROOT) else {
            continue;
        };

        let Some((category, _)) = path.split_once('/') else {
            continue;
        };

        if !matches!(
            category,
            "classes" | "datatypes" | "enums" | "globals" | "libraries"
        ) || !path.ends_with(".yaml")
        {
            continue;
        }

        let address = format!(
            "https://raw.githubusercontent.com/Roblox/creator-docs/{}/{}",
            tree.sha, file.path
        );

        let client = client.clone();
        let path = path.to_owned();

        pending.spawn(async move {
            download(&client, &address)
                .await
                .map(|source| (path, source))
        });

        if pending.len() == 16 {
            let (path, source) = pending
                .join_next()
                .await
                .ok_or_else(|| invalid("Missing Creator Docs download."))?
                .map_err(|error| invalid(format!("Creator Docs download failed. {error}")))??;

            sources.insert(path, source);
        }
    }

    while let Some(result) = pending.join_next().await {
        let (path, source) =
            result.map_err(|error| invalid(format!("Creator Docs download failed. {error}")))??;

        sources.insert(path, source);
    }

    if sources.is_empty() {
        return Err(invalid("Creator Docs contains no engine reference files."));
    }

    let mut documents = BTreeMap::new();

    for (path, source) in sources {
        let entry: Entry = yaml_serde::from_slice(&source)
            .map_err(|error| invalid(format!("Invalid Creator Docs `{path}`. {error}")))?;

        import(&path, &entry, &mut documents)?;
    }

    Ok(documents)
}

fn import(
    path: &str,
    entry: &Entry,
    documents: &mut BTreeMap<String, Vec<Document>>,
) -> Result<(), Error> {
    let (category, filename) = path
        .split_once('/')
        .ok_or_else(|| invalid("Invalid Creator Docs path."))?;

    let base = Url::parse(&format!(
        "https://create.roblox.com/docs/reference/engine/{category}/{}",
        filename.trim_end_matches(".yaml")
    ))
    .map_err(|error| invalid(format!("Invalid Creator Docs address. {error}")))?;

    let namespace = match category {
        "classes" | "datatypes" => "globaltype",
        "enums" => "enum",
        _ => "global",
    };

    if category != "globals" {
        documents
            .entry(format!("@roblox/{namespace}/{}", entry.name))
            .or_default()
            .push(document(entry, &base)?);

        if category == "datatypes" {
            documents
                .entry(format!("@roblox/global/{}", entry.name))
                .or_default()
                .push(document(entry, &base)?);
        }
    }

    for kind in [
        "properties",
        "methods",
        "events",
        "callbacks",
        "items",
        "constructors",
        "constants",
        "functions",
    ] {
        let Some(value) = entry.fields.get(kind).filter(|value| !value.is_null()) else {
            continue;
        };

        let members: Vec<Entry> = serde_json::from_value(value.clone())
            .map_err(|error| invalid(format!("Invalid Creator Docs `{path}` {kind}. {error}")))?;

        for member in members {
            let namespace = if category == "datatypes"
                && matches!(kind, "constructors" | "functions" | "constants")
            {
                "global"
            } else {
                namespace
            };

            let name = member.name.replace(':', ".");
            let symbol = format!("@roblox/{namespace}/{name}");
            let mut address = base.clone();

            address.set_fragment(Some(
                member
                    .name
                    .rsplit(['.', ':'])
                    .next()
                    .unwrap_or(&member.name)
                    .trim_end_matches("()"),
            ));

            documents
                .entry(symbol)
                .or_default()
                .push(document(&member, &address)?);
        }
    }

    Ok(())
}

fn document(entry: &Entry, address: &Url) -> Result<Document, Error> {
    if entry.name.trim().is_empty() {
        return Err(invalid(format!(
            "Unnamed Creator Docs entry at `{address}`."
        )));
    }

    Ok(Document {
        documentation: markup(entry.prose()),
        link: address.to_string(),
        parameters: entry.parameters.as_ref().map(|parameters| {
            parameters
                .iter()
                .map(|parameter| Parameter {
                    name: parameter.name.clone(),
                    documentation: markup(parameter.prose()),
                })
                .collect()
        }),
        returns: entry.returns.as_ref().map(|returns| {
            returns
                .iter()
                .map(|returned| markup(returned.prose()))
                .collect()
        }),
        has_receiver: entry.name.contains(':'),
    })
}

fn markup(text: &str) -> String {
    let options = Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH;
    let mut events = Vec::new();

    for event in Parser::new_ext(text, options) {
        match event {
            Event::Code(text) => {
                let (reference, label) = text
                    .split_once('|')
                    .map_or((text.as_ref(), None), |(reference, label)| {
                        (reference, Some(label))
                    });

                let target = reference.split_once('.').and_then(|(kind, name)| {
                    let directory = match kind {
                        "Class" => "classes",
                        "Datatype" => "datatypes",
                        "Enum" => "enums",
                        "Library" => "libraries",
                        "Global" => "globals",
                        _ => return None,
                    };

                    Some((directory, name))
                });

                if let Some((directory, name)) = target {
                    let display = label
                        .filter(|label| *label != "no-link")
                        .unwrap_or(name)
                        .to_owned();

                    let (owner, member) = name
                        .split_once(['.', ':'])
                        .map_or((name, None), |(owner, member)| (owner, Some(member)));

                    let mut address = format!(
                        "https://create.roblox.com/docs/reference/engine/{directory}/{owner}"
                    );

                    if let Some(member) = member {
                        address.push('#');
                        address.push_str(member.trim_end_matches("()"));
                    }

                    if label != Some("no-link") {
                        events.push(Event::Start(Tag::Link {
                            link_type: LinkType::Inline,
                            dest_url: address.into(),
                            title: "".into(),
                            id: "".into(),
                        }));
                    }

                    events.push(Event::Code(display.into()));

                    if label != Some("no-link") {
                        events.push(Event::End(TagEnd::Link));
                    }
                } else {
                    events.push(Event::Code(text));
                }
            }

            Event::Start(mut tag) => {
                if let Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } = &mut tag {
                    let boundary = dest_url.find(['?', '#']).unwrap_or(dest_url.len());

                    if let Some(path) = dest_url[..boundary].strip_suffix(".md") {
                        *dest_url = format!("{path}{}", &dest_url[boundary..]).into();
                    }
                }

                events.push(Event::Start(tag));
            }

            _ => events.push(event),
        }
    }

    let mut result = String::new();
    html::push_html(&mut result, events.into_iter());

    result
}
