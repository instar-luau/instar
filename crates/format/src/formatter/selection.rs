use std::{io, ops::Range, time::Instant};

use instar_analysis::Options;

use vermis::{
    token::TokenKind,
    tree::{ListEntry, NodeIndex, NodeKind, NodeList, Tree},
};

use crate::Configuration;

pub(super) struct Selection {
    pub(super) statements: Vec<ListEntry>,
    pub(super) range: Range<usize>,
    pub(super) indentation: usize,
}

impl Selection {
    pub(super) fn new(
        tree: &Tree<'_>,
        requested: [usize; 2],
        configuration: &Configuration,
        options: &Options,
        started: Instant,
    ) -> io::Result<Option<Self>> {
        if tree.source.get(requested[0]..requested[1]).is_none() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid formatting range",
            ));
        }

        let mut range = lines(tree.source, requested[0]..requested[1]);

        loop {
            options.check(started)?;

            let mut tokens = tree.tokens.iter().filter(|token| {
                token.span.start < range.end
                    && range.start < token.span.end
                    && !matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile)
            });

            let Some(first) = tokens.next() else {
                return Ok(None);
            };

            let last = tokens.next_back().unwrap_or(first);
            let mut selected = None;
            let mut size = usize::MAX;
            let mut pending = vec![(tree.root, None)];

            while let Some((index, parent)) = pending.pop() {
                options.check(started)?;
                let node = tree.node(index);

                if let NodeKind::Block { statements } = &node.kind {
                    let root = parent.is_some_and(|parent| {
                        matches!(tree.node(parent).kind, NodeKind::Root { .. })
                    });

                    if (root
                        || node.span.start <= first.span.start && last.span.end <= node.span.end)
                        && node.span.end - node.span.start <= size
                        && let Some(candidate) =
                            select(tree, statements, parent, range.clone(), configuration)
                    {
                        selected = Some(candidate);
                        size = node.span.end - node.span.start;
                    }
                }

                pending.extend(
                    tree.children(index)
                        .into_iter()
                        .map(|child| (child, Some(index))),
                );
            }

            let Some(mut selected) = selected else {
                return Ok(None);
            };

            let expanded = lines(
                tree.source,
                selected.range.start.min(first.span.start).min(range.start)
                    ..selected.range.end.max(last.span.end).max(range.end),
            );

            if expanded != range {
                range = expanded;
                continue;
            }

            selected.range = range;
            let parsed = vermis::parse(&tree.source[selected.range.clone()]);
            options.check(started)?;

            if !parsed.diagnostics.is_empty() {
                return Ok(None);
            }

            return Ok(Some(selected));
        }
    }

    pub(super) fn tokens(&self, tree: &Tree<'_>) -> Range<usize> {
        tree.tokens
            .partition_point(|token| token.span.end <= self.range.start)
            ..tree
                .tokens
                .partition_point(|token| token.span.start < self.range.end)
    }
}

fn select(
    tree: &Tree<'_>,
    statements: &NodeList,
    parent: Option<NodeIndex>,
    range: Range<usize>,
    configuration: &Configuration,
) -> Option<Selection> {
    let statements = tree.list(statements);

    let first = statements
        .iter()
        .position(|entry| tree.node(entry.node).span.end > range.start)?;

    let last = statements
        .iter()
        .rposition(|entry| tree.node(entry.node).span.start < range.end)?;

    if first > last {
        return None;
    }

    let beginning = tree.node(statements[first].node).span.start;

    let ending = statements[last].separator.map_or_else(
        || tree.node(statements[last].node).span.end,
        |separator| tree.token(separator).span.end,
    );

    let indentation = parent
        .filter(|&parent| !matches!(tree.node(parent).kind, NodeKind::Root { .. }))
        .map_or(0, |parent| {
            indentation(
                tree.source,
                tree.node(parent).span.start,
                configuration.indent_width.get(),
            ) + 1
        });

    Some(Selection {
        statements: statements[first..=last].to_vec(),
        range: beginning..ending,
        indentation,
    })
}

fn lines(source: &[u8], range: Range<usize>) -> Range<usize> {
    let start = source[..range.start]
        .iter()
        .rposition(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(0, |index| index + 1);

    let mut end = range.end;

    if end == range.start || end == 0 || !matches!(source[end - 1], b'\r' | b'\n') {
        while end < source.len() && !matches!(source[end], b'\r' | b'\n') {
            end += 1;
        }

        if source.get(end) == Some(&b'\r') {
            end += 1;
        }

        if source.get(end) == Some(&b'\n') {
            end += 1;
        }
    } else if source.get(end - 1) == Some(&b'\r') && source.get(end) == Some(&b'\n') {
        end += 1;
    }

    start..end
}

fn indentation(source: &[u8], offset: usize, width: usize) -> usize {
    let start = source[..offset]
        .iter()
        .rposition(|byte| matches!(byte, b'\r' | b'\n'))
        .map_or(0, |index| index + 1);

    source[start..offset]
        .iter()
        .take_while(|byte| matches!(byte, b' ' | b'\t'))
        .fold(0, |column, byte| {
            if *byte == b'\t' {
                column + width - column % width
            } else {
                column + 1
            }
        })
        / width
}
