use pulldown_cmark::{Event, Parser, Tag};
use serde_json::{Value, json};

use super::{Analysis, Symbol, TokenKind};

impl Analysis {
    /// Returns leading declaration comments in the documentation catalog schema.
    #[must_use]
    pub fn documentation(&self, offset: usize) -> Option<Value> {
        let offset = declaration(&self.symbols, offset).unwrap_or(offset);
        let source = &self.source;

        let mut end = source
            .get(..offset)?
            .rfind('\n')
            .map_or(0, |index| index + 1);

        let preceding = self.tokens.partition_point(|token| token.range.end <= end);
        let mut lines = Vec::new();

        for token in self.tokens[..preceding].iter().rev() {
            if token.kind != TokenKind::Comment {
                break;
            }

            let gap = &source[token.range.end..end];

            let start = source[..token.range.start]
                .rfind('\n')
                .map_or(usize::from(source.starts_with('\u{feff}')) * 3, |index| {
                    index + 1
                });

            if !gap.trim().is_empty()
                || gap.bytes().filter(|byte| *byte == b'\n').count() != 1
                || !source[start..token.range.start].trim().is_empty()
            {
                break;
            }

            let comment = &source[token.range.clone()];

            if comment.starts_with("--!") {
                break;
            }

            let block = comment
                .strip_prefix("--[")
                .and_then(|text| text.split_once('['))
                .filter(|(level, _)| level.bytes().all(|byte| byte == b'='))
                .and_then(|(level, text)| text.strip_suffix(&format!("]{level}]")));

            let text = block.unwrap_or_else(|| {
                let text = comment
                    .strip_prefix("---")
                    .or_else(|| comment.strip_prefix("--"))
                    .unwrap_or_default();

                text.strip_prefix(' ').unwrap_or(text)
            });

            lines.push(text);
            end = start;
        }

        lines.reverse();
        let text = lines.join("\n");

        let indentation = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start_matches([' ', '\t']).len())
            .min()
            .unwrap_or(0);

        let text = text
            .lines()
            .map(|line| line.get(indentation..).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n");

        metadata(text.trim())
    }
}

fn declaration(symbols: &[Symbol], offset: usize) -> Option<usize> {
    symbols.iter().find_map(|symbol| {
        if symbol.selection.contains(&offset) {
            Some(symbol.range.start)
        } else {
            declaration(&symbol.children, offset)
        }
    })
}

fn metadata(text: &str) -> Option<Value> {
    let mut blocks = Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::Start(Tag::CodeBlock(_))).then_some(range)
        })
        .peekable();

    let mut description = Vec::new();
    let mut parameters = Vec::new();
    let mut returns = Vec::new();
    let mut offset = 0;

    for line in text.split_inclusive('\n') {
        while blocks.peek().is_some_and(|range| range.end <= offset) {
            blocks.next();
        }

        let code = blocks
            .peek()
            .is_some_and(|range| range.start < offset + line.len());

        offset += line.len();
        let line = line.trim_end_matches('\n');
        let tag = line.trim_start();
        let (tag, content) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let content = content.trim();

        match (code, tag) {
            (false, "@param") if !content.is_empty() => {
                let (name, description) = content
                    .split_once(char::is_whitespace)
                    .unwrap_or((content, ""));

                parameters.push(json!({"name": name, "documentation": description.trim()}));
            }

            (false, "@return" | "@returns") if !content.is_empty() => {
                returns.push(json!({"documentation": content}));
            }

            (false, "@within" | "@method" | "@function" | "@class" | "@interface") => {}

            (false, tag) if tag.starts_with('@') => {
                description.push(format!("`{tag}` {content}").trim_end().to_owned());
            }

            _ => description.push(line.to_owned()),
        }
    }

    let description = description.join("\n").trim().to_owned();

    (!description.is_empty() || !parameters.is_empty() || !returns.is_empty())
        .then(|| json!({"documentation": description, "params": parameters, "returns": returns}))
}
