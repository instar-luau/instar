use std::ops::Range;

use vermis::{
    token::TokenKind,
    tree::{ListEntry, Tree},
};

pub(super) fn ranges(tree: &Tree<'_>, entries: &[ListEntry]) -> Vec<Range<usize>> {
    let mut result = Vec::with_capacity(entries.len());

    for (position, entry) in entries.iter().enumerate() {
        let node = tree.node(entry.node);

        let previous = position
            .checked_sub(1)
            .map(|previous| finish(tree, &entries[previous]));

        let lower = previous.map_or_else(
            || {
                tree.tokens[..node.tokens.start.get()]
                    .iter()
                    .rev()
                    .find(|token| {
                        !matches!(
                            token.kind,
                            TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                        )
                    })
                    .map_or(0, |token| line_end(tree.source, token.span.end))
            },
            |end| line_end(tree.source, end),
        );

        let mut start = node.span.start;

        for token in &tree.tokens[..node.tokens.start.get()] {
            if token.span.start < lower {
                continue;
            }

            if matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
                && !token.bytes(tree.source).starts_with(b"--!")
            {
                start = token.span.start;
                break;
            }
        }

        let end = finish(tree, entry);

        result.push(line_start(tree.source, start)..line_end(tree.source, end));
    }

    result
}

pub(super) fn isolated(
    tree: &Tree<'_>,
    entries: &[ListEntry],
    selected: &Range<usize>,
    boundary: &Range<usize>,
) -> bool {
    let first = tree.node(entries[selected.start].node).span.start;
    let entry = &entries[selected.end - 1];

    let last = entry
        .separator
        .map_or(tree.node(entry.node).span.end, |separator| {
            tree.token(separator).span.end
        });

    !tree.tokens.iter().any(|token| {
        token.span.start < boundary.end
            && boundary.start < token.span.end
            && (token.span.start < first || last < token.span.end)
            && !matches!(
                token.kind,
                TokenKind::Whitespace
                    | TokenKind::Comment
                    | TokenKind::BlockComment
                    | TokenKind::EndOfFile
            )
    })
}

fn line_start(source: &[u8], position: usize) -> usize {
    source[..position]
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1)
}

fn finish(tree: &Tree<'_>, entry: &ListEntry) -> usize {
    let node = tree.node(entry.node);

    let position = entry
        .separator
        .map_or(node.tokens.end.get(), |separator| separator.get() + 1);

    let mut end = entry
        .separator
        .map_or(node.span.end, |separator| tree.token(separator).span.end);

    for token in &tree.tokens[position..] {
        match token.kind {
            TokenKind::Whitespace if !token.bytes(tree.source).contains(&b'\n') => {}
            TokenKind::Comment | TokenKind::BlockComment => end = token.span.end,
            _ => break,
        }
    }

    end
}

fn line_end(source: &[u8], position: usize) -> usize {
    source[position..]
        .iter()
        .position(|byte| *byte == b'\n')
        .map_or(source.len(), |end| position + end + 1)
}
