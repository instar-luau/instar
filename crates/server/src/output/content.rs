use std::mem;

use pulldown_cmark::{CodeBlockKind, Event, LinkType, Options, Parser, Tag, TagEnd, html};

use pulldown_cmark_to_cmark::{calculate_code_block_token_count, cmark};

use scraper::{ElementRef, Html};
use tower_lsp_server::{gen_lsp_types::Uri, jsonrpc::Result};

use crate::error::failure;

type Inline = Vec<Event<'static>>;

enum Block {
    Paragraph(Inline),
    Code(String),

    List {
        start: Option<u64>,
        items: Vec<Vec<Self>>,
    },

    Separator,
}

struct Source {
    attribute: String,
    samples: Vec<String>,
}

impl Source {
    fn retain(&mut self, text: String, tag: &str) -> String {
        let index = self.samples.len();
        self.samples.push(text);

        format!("<{tag} {}=\"{index}\"></{tag}>", self.attribute)
    }

    fn sample(&self, element: ElementRef<'_>) -> Option<&str> {
        element
            .attr(&self.attribute)
            .and_then(|index| index.parse::<usize>().ok())
            .and_then(|index| self.samples.get(index))
            .map(String::as_str)
    }
}

pub(in crate::output) fn render(text: &str, should_use_markdown: bool) -> Result<String> {
    if text.is_empty() {
        return Ok(String::new());
    }

    let mut retained = Source {
        attribute: "data-instar-code".to_owned(),
        samples: Vec::new(),
    };

    let lowercase = text.to_ascii_lowercase();

    while lowercase.contains(&retained.attribute) {
        retained.attribute.push('x');
    }

    let mut source = String::new();

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_GFM;

    let mut parser = Parser::new_ext(text, options);
    let mut protected = Vec::new();

    while let Some(event) = parser.next() {
        match event {
            Event::Start(Tag::CodeBlock(_)) => {
                let mut sample = String::new();

                for event in parser.by_ref() {
                    match event {
                        Event::Text(text) => sample.push_str(&text),
                        Event::End(TagEnd::CodeBlock) => break,
                        _ => {}
                    }
                }

                protected.push(Event::Html(retained.retain(sample, "pre").into()));
            }

            Event::Code(text) => {
                protected.push(Event::InlineHtml(
                    retained.retain(text.into_string(), "code").into(),
                ));
            }

            event => protected.push(event.into_static()),
        }
    }

    html::push_html(&mut source, protected.into_iter());
    let document = Html::parse_fragment(&source);
    let mut events = Vec::new();

    flatten(
        &blocks(document.root_element(), &retained),
        "",
        false,
        &mut events,
    );

    serialize(&events, should_use_markdown)
}

pub(in crate::output) fn code(text: &str, should_use_markdown: bool) -> Result<String> {
    if !should_use_markdown {
        return Ok(text.to_owned());
    }

    serialize(
        &[
            Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced("luau".into()))),
            Event::Text(text.to_owned().into()),
            Event::End(TagEnd::CodeBlock),
        ],
        true,
    )
}

pub(in crate::output) fn link(label: &str, url: &str, should_use_markdown: bool) -> Result<String> {
    let mut events = vec![
        Event::Start(Tag::Paragraph),
        Event::Text(format!("{label} (").into()),
    ];

    events.extend(destination(url));
    events.push(Event::Text(")".into()));
    events.push(Event::End(TagEnd::Paragraph));

    serialize(&events, should_use_markdown)
}

fn blocks(element: ElementRef<'_>, source: &Source) -> Vec<Block> {
    let mut output = Vec::new();
    let mut pending = Vec::new();

    for child in element.children() {
        if let Some(text) = child.value().as_text() {
            pending.push(Event::Text(whitespace(text).into()));
            continue;
        }

        let Some(child) = ElementRef::wrap(child) else {
            continue;
        };

        match child.value().name() {
            "pre" => {
                paragraph(&mut output, &mut pending);

                output.push(Block::Code(
                    source
                        .sample(child)
                        .map_or_else(|| child.text().collect(), str::to_owned),
                ));
            }

            "ul" | "ol" => {
                paragraph(&mut output, &mut pending);

                output.push(Block::List {
                    start: (child.value().name() == "ol").then(|| {
                        child
                            .attr("start")
                            .and_then(|start| start.parse().ok())
                            .unwrap_or(1)
                    }),
                    items: child
                        .child_elements()
                        .map(|item| blocks(item, source))
                        .collect(),
                });
            }

            "table" => {
                paragraph(&mut output, &mut pending);
                output.extend(table(child, source));
            }

            "hr" => {
                paragraph(&mut output, &mut pending);
                output.push(Block::Separator);
            }

            name if is_block(name)
                || child
                    .descendent_elements()
                    .any(|element| is_block(element.value().name())) =>
            {
                paragraph(&mut output, &mut pending);
                output.extend(blocks(child, source));
            }

            _ => pending.extend(inline(child, source)),
        }
    }

    paragraph(&mut output, &mut pending);

    output
}

fn is_block(name: &str) -> bool {
    matches!(
        name,
        "address"
            | "article"
            | "aside"
            | "blockquote"
            | "center"
            | "details"
            | "dialog"
            | "div"
            | "dl"
            | "dt"
            | "dd"
            | "fieldset"
            | "figure"
            | "figcaption"
            | "footer"
            | "form"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "header"
            | "hgroup"
            | "hr"
            | "legend"
            | "li"
            | "main"
            | "menu"
            | "nav"
            | "ol"
            | "p"
            | "pre"
            | "section"
            | "summary"
            | "table"
            | "ul"
    )
}

fn paragraph(output: &mut Vec<Block>, pending: &mut Inline) {
    if plain(pending).trim().is_empty() {
        pending.clear();
    } else {
        output.push(Block::Paragraph(mem::take(pending)));
    }
}

fn inline(element: ElementRef<'_>, source: &Source) -> Inline {
    let name = element.value().name();

    if name == "code" {
        return vec![Event::Code(
            source
                .sample(element)
                .map_or_else(|| element.text().collect::<String>(), str::to_owned)
                .into(),
        )];
    }

    if name == "br" {
        return vec![Event::SoftBreak];
    }

    if name == "input" {
        return vec![Event::Text(
            if element.attr("checked").is_some() {
                "(complete) "
            } else {
                "(incomplete) "
            }
            .into(),
        )];
    }

    let mut output = Vec::new();

    let style = match name {
        "strong" | "b" => Some(Tag::Strong),
        "em" | "i" => Some(Tag::Emphasis),
        _ => None,
    };

    if let Some(style) = &style {
        output.push(Event::Start(style.clone()));
    }

    for child in element.children() {
        if let Some(text) = child.value().as_text() {
            output.push(Event::Text(whitespace(text).into()));
        } else if let Some(child) = ElementRef::wrap(child) {
            output.extend(inline(child, source));
        }
    }

    if let Some(style) = style {
        output.push(Event::End(style.to_end()));
    }

    if name == "img" {
        output.push(Event::Text(
            element.attr("alt").unwrap_or_default().to_owned().into(),
        ));
    }

    if let Some(url) = match name {
        "a" => element.attr("href"),
        "img" => element.attr("src"),
        _ => None,
    }
    .filter(|url| !url.is_empty())
    {
        let label = plain(&output);

        if label.trim() == url {
            output = destination(url);
        } else {
            if !label.trim().is_empty() {
                output.push(Event::Text(" (".into()));
            }

            output.extend(destination(url));

            if !label.trim().is_empty() {
                output.push(Event::Text(")".into()));
            }
        }
    }

    if let Some(title) = element.attr("title").filter(|title| !title.is_empty())
        && plain(&output).trim() != title
    {
        output.push(Event::Text(format!(" — {title}").into()));
    }

    output
}

fn destination(url: &str) -> Inline {
    let Some(uri) = url
        .parse::<Uri>()
        .ok()
        .filter(|uri| matches!(uri.scheme(), "http" | "https"))
    else {
        return vec![Event::Text(url.to_owned().into())];
    };

    vec![
        Event::Start(Tag::Link {
            link_type: if uri.as_str() == url {
                LinkType::Autolink
            } else {
                LinkType::Inline
            },
            dest_url: uri.to_string().into(),
            title: "".into(),
            id: "".into(),
        }),
        Event::Text(url.to_owned().into()),
        Event::End(TagEnd::Link),
    ]
}

fn table(element: ElementRef<'_>, source: &Source) -> Vec<Block> {
    let mut output = Vec::new();
    let mut headers = Vec::new();

    let has_spans = element
        .descendent_elements()
        .any(|child| child.attr("rowspan").is_some() || child.attr("colspan").is_some());

    let mut row_number = 0;

    for child in element.descendent_elements().filter(|child| {
        child
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|ancestor| ancestor.value().name() == "table")
            == Some(element)
    }) {
        if child.value().name() == "caption" {
            output.extend(blocks(child, source));
        }

        if child.value().name() != "tr" {
            continue;
        }

        row_number += 1;

        let cells: Vec<_> = child.child_elements().collect();
        let is_heading = cells.iter().all(|cell| cell.value().name() == "th");

        if is_heading && !has_spans {
            let mut row = Vec::new();

            headers = cells
                .iter()
                .map(|cell| plain(&inline(*cell, source)))
                .collect::<Vec<_>>();

            for (index, cell) in cells.iter().enumerate() {
                if index != 0 {
                    row.push(Event::Text("; ".into()));
                }

                row.extend(inline(*cell, source));
            }

            if !row.is_empty() {
                output.push(Block::Paragraph(row));
            }
        } else {
            if !output.is_empty() {
                output.push(Block::Separator);
            }

            for (index, cell) in cells.iter().enumerate() {
                let mut contents = blocks(*cell, source);

                let label = if has_spans {
                    let kind = if cell.value().name() == "th" {
                        "header"
                    } else {
                        "entry"
                    };

                    let mut label = format!("Row {row_number}, {kind} {}", index + 1);

                    for attribute in ["rowspan", "colspan"] {
                        if let Some(value) = cell.attr(attribute) {
                            label.push_str(" (");
                            label.push_str(attribute);
                            label.push_str(": ");
                            label.push_str(value);
                            label.push(')');
                        }
                    }

                    Some(label)
                } else {
                    headers.get(index).cloned()
                };

                if let Some(label) = label {
                    let prefix = Event::Text(format!("{label}: ").into());

                    if let Some(Block::Paragraph(content)) = contents.first_mut() {
                        content.insert(0, prefix);
                    } else {
                        contents.insert(0, Block::Paragraph(vec![prefix]));
                    }
                }

                output.extend(contents);
            }
        }
    }

    output
}

fn flatten(blocks: &[Block], context: &str, is_item: bool, output: &mut Inline) {
    let label = blocks
        .iter()
        .find_map(|block| match block {
            Block::Paragraph(events) => Some(plain(events)),
            _ => None,
        })
        .unwrap_or_default();

    for block in blocks {
        match block {
            Block::Paragraph(events) => {
                if is_item {
                    output.push(Event::Start(Tag::List(None)));
                    output.push(Event::Start(Tag::Item));
                } else {
                    output.push(Event::Start(Tag::Paragraph));
                }

                if !context.is_empty() {
                    output.push(Event::Text(context.to_owned().into()));
                }

                output.extend(events.iter().cloned());

                if is_item {
                    output.push(Event::End(TagEnd::Item));
                    output.push(Event::End(TagEnd::List(false)));
                } else {
                    output.push(Event::End(TagEnd::Paragraph));
                }
            }

            Block::Code(text) => {
                output.push(Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(
                    "luau".into(),
                ))));

                output.push(Event::Text(text.clone().into()));
                output.push(Event::End(TagEnd::CodeBlock));
            }

            Block::Separator => output.push(Event::Rule),

            Block::List { start, items } => {
                let parent = if is_item && !label.trim().is_empty() {
                    format!("{context}{} — ", label.trim())
                } else {
                    context.to_owned()
                };

                let mut number = *start;

                for contents in items {
                    let prefix = number
                        .map_or_else(|| parent.clone(), |number| format!("{parent}{number}: "));

                    flatten(contents, &prefix, true, output);
                    number = number.map(|number| number.saturating_add(1));
                }
            }
        }
    }
}

fn serialize(events: &[Event<'_>], should_use_markdown: bool) -> Result<String> {
    if !should_use_markdown {
        let mut output = String::new();
        let mut should_separate = false;

        for event in events {
            match event {
                Event::Start(Tag::Item) => {
                    if should_separate {
                        separate(&mut output);
                    }

                    output.push_str("• ");
                    should_separate = false;
                }

                Event::Text(text) | Event::Code(text) => {
                    if should_separate {
                        separate(&mut output);
                    }

                    output.push_str(text);
                    should_separate = false;
                }

                Event::SoftBreak | Event::HardBreak => output.push('\n'),

                Event::End(TagEnd::Paragraph | TagEnd::Item | TagEnd::CodeBlock) | Event::Rule => {
                    should_separate = true;
                }

                _ => {}
            }
        }

        return Ok(output);
    }

    let mut output = String::new();

    let fence = "`".repeat(calculate_code_block_token_count(events).unwrap_or(3).max(3));
    let mut pending = Vec::new();
    let mut remaining = events.iter();

    while let Some(event) = remaining.next() {
        if let Event::Start(Tag::CodeBlock(kind)) = event {
            cmark(pending.drain(..), &mut output, Options::empty()).map_err(failure)?;
            separate(&mut output);
            output.push_str(&fence);

            output.push_str(match kind {
                CodeBlockKind::Fenced(language) => language,
                CodeBlockKind::Indented => "luau",
            });

            output.push('\n');

            for event in remaining.by_ref() {
                match event {
                    Event::Text(text) => output.push_str(text),
                    Event::End(TagEnd::CodeBlock) => break,
                    _ => {}
                }
            }

            if !output.ends_with('\n') {
                output.push('\n');
            }

            output.push_str(&fence);
            separate(&mut output);
        } else {
            pending.push(event);
        }
    }

    cmark(pending, &mut output, Options::empty()).map_err(failure)?;

    Ok(output.trim().to_owned())
}

fn separate(output: &mut String) {
    if !output.is_empty() {
        if !output.ends_with('\n') {
            output.push('\n');
        }

        if !output.ends_with("\n\n") {
            output.push('\n');
        }
    }
}

fn plain(events: &[Event<'_>]) -> String {
    let mut output = String::new();

    for event in events {
        match event {
            Event::Text(text) | Event::Code(text) => output.push_str(text),
            Event::SoftBreak | Event::HardBreak => output.push(' '),
            _ => {}
        }
    }

    output
}

fn whitespace(text: &str) -> String {
    let mut output = String::new();

    for character in text.chars() {
        if !matches!(character, ' ' | '\t' | '\r' | '\n' | '\u{000c}') {
            output.push(character);
        } else if !output.ends_with(' ') {
            output.push(' ');
        }
    }

    output
}
