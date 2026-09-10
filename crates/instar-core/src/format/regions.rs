use std::ops::Range;

use vermis::{TokenKind, Tree};

pub(super) fn held(source: &str, tree: &Tree<'_>) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open = None;

    for token in &tree.tokens {
        if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            continue;
        }

        let text = &source[token.span.start..token.span.end];

        let instruction = text
            .split_once("instar:")
            .and_then(|(_, rest)| rest.trim().strip_prefix("format"))
            .map(str::trim);

        let start = source[..token.span.start]
            .rfind('\n')
            .map_or(0, |position| position + 1);

        match instruction {
            Some("off") if open.is_none() => open = Some(start),

            Some("on") => {
                if let Some(start) = open.take() {
                    ranges.push(start..token.span.end);
                }
            }

            Some(instruction) => {
                if let Some(count) = instruction
                    .strip_prefix("off(")
                    .and_then(|tail| tail.strip_suffix(')'))
                    .and_then(|count| count.trim().parse::<usize>().ok())
                {
                    let mut end = token.span.end;

                    for _ in 0..=count {
                        let Some(position) = source[end..].find('\n') else {
                            end = source.len();
                            break;
                        };

                        end += position + 1;
                    }

                    ranges.push(start..end);
                }
            }

            _ => {}
        }
    }

    if let Some(start) = open {
        ranges.push(start..source.len());
    }

    ranges
}
