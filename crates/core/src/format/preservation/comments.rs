use std::{
    collections::{BTreeMap, BTreeSet},
    iter::repeat_n,
};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, TokenIndex, Tree},
};

use super::syntax::substantive;

use crate::{
    format::{Error, directives::Directives},
    settings::format::{Configuration, Indentation},
};

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Location {
    Global,

    Interior {
        node: usize,
        gap: usize,
    },

    Boundary {
        marker: usize,
        node: usize,
        gap: usize,
        token: usize,
        inline: bool,
    },

    Adjacent {
        node: usize,
        gap: usize,
        token: usize,
        inline: bool,
    },

    Detached {
        inline: bool,
    },
}

pub(super) struct Attachment {
    location: Location,
    token: TokenIndex,
}

struct Comment {
    position: usize,
    adjacent: Option<usize>,
    inline: bool,
    global: bool,
}

pub(super) fn attached(
    tree: &Tree<'_>,
    correspondence: Option<&[Option<NodeIndex>]>,
    ordering: bool,
    directives: &Directives,
) -> Option<Vec<Attachment>> {
    let mut comments = Vec::new();
    let mut previous = None;
    let mut leading = true;

    for (position, token) in tree.tokens.iter().enumerate() {
        if matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
            let inline = previous.is_some_and(|previous: usize| {
                !tree.source[tree.tokens[previous].span.end..token.span.start].contains(&b'\n')
            });

            comments.push(Comment {
                position,
                adjacent: if inline { previous } else { None },
                inline,
                global: leading && token.bytes(tree.source).starts_with(b"--!"),
            });

            if directives.marker(position).is_some() {
                previous = Some(position);
            }
        } else {
            if !matches!(token.kind, TokenKind::Whitespace | TokenKind::EndOfFile) {
                leading = false;
            }

            if substantive(token.kind) {
                previous = Some(position);
            }
        }
    }

    if comments.is_empty() {
        return Some(Vec::new());
    }

    let mut next = None;
    let mut remaining = comments.iter_mut().rev().peekable();

    for (position, token) in tree.tokens.iter().enumerate().rev() {
        if remaining
            .peek()
            .is_some_and(|comment| comment.position == position)
        {
            let comment = remaining.next()?;

            if !comment.inline {
                comment.adjacent = next;
            }

            if directives.marker(position).is_some() {
                next = Some(position);
            }
        } else if substantive(token.kind) {
            next = Some(position);
        }
    }

    let targets: BTreeSet<_> = comments
        .iter()
        .filter(|comment| !comment.global)
        .flat_map(|comment| {
            [Some(comment.position), comment.adjacent]
                .into_iter()
                .flatten()
        })
        .collect();

    let owners = owners(tree, &targets)?;
    let mut children = BTreeMap::new();
    let mut anchors = BTreeMap::new();
    let mut result = Vec::with_capacity(comments.len());

    for comment in comments {
        let marker = directives.marker(comment.position);

        let location = if comment.global {
            Location::Global
        } else {
            let owner = *owners.get(&comment.position)?;

            if marker.is_none() && interior(&tree.node(owner).kind) {
                let children = children
                    .entry(owner.get())
                    .or_insert_with(|| tree.children(owner));

                let gap = children.partition_point(|child| {
                    tree.node(*child).tokens.end.get() <= comment.position
                });

                Location::Interior {
                    node: identity(owner, correspondence)?,
                    gap,
                }
            } else if let Some(adjacent) = marker.map(|_| comment.position).or(comment.adjacent) {
                let (node, gap, token) = if let Some(anchor) = anchors.get(&adjacent) {
                    *anchor
                } else {
                    let owner = *owners.get(&adjacent)?;

                    let children = children
                        .entry(owner.get())
                        .or_insert_with(|| tree.children(owner));

                    let gap = children
                        .partition_point(|child| tree.node(*child).tokens.end.get() <= adjacent);

                    let start = gap
                        .checked_sub(1)
                        .map_or(tree.node(owner).tokens.start.get(), |previous| {
                            tree.node(children[previous]).tokens.end.get()
                        });

                    let token = tree
                        .tokens
                        .get(start..=adjacent)?
                        .iter()
                        .filter(|token| substantive(token.kind))
                        .count();

                    let node = identity(owner, correspondence)?;
                    anchors.insert(adjacent, (node, gap, token));

                    (node, gap, token)
                };

                if let Some(marker) = marker {
                    Location::Boundary {
                        marker,
                        node,
                        gap,
                        token,
                        inline: comment.inline,
                    }
                } else {
                    Location::Adjacent {
                        node,
                        gap,
                        token,
                        inline: comment.inline,
                    }
                }
            } else {
                Location::Detached {
                    inline: comment.inline,
                }
            }
        };

        result.push(Attachment {
            location,
            token: TokenIndex::new(comment.position),
        });
    }

    if ordering {
        result.sort_by_key(|attachment| attachment.location);
    }

    Some(result)
}

pub(crate) fn reindent(
    tree: &Tree<'_>,
    index: TokenIndex,
    target_columns: usize,
    configuration: &Configuration,
) -> Option<Vec<u8>> {
    standalone(tree, index, configuration)?;

    let bytes = tree.token(index).bytes(tree.source);
    let delimiter = 2 + bytes.get(3..)?.iter().position(|&byte| byte != b'=')?;
    let closing = bytes.len().checked_sub(delimiter)?;
    let beginning = bytes[..closing].iter().rposition(|byte| *byte == b'\n')? + 1;

    if !bytes[beginning..closing]
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t'))
    {
        return None;
    }

    let mut result = bytes[..beginning].to_vec();

    let spaces = match configuration.indent_style {
        Indentation::Tabs => {
            let width = configuration.indent_width.get();
            result.extend(repeat_n(b'\t', target_columns / width));

            target_columns % width
        }

        Indentation::Spaces => target_columns,
    };

    result.extend(repeat_n(b' ', spaces));
    result.extend_from_slice(&bytes[closing..]);

    (result != bytes).then_some(result)
}

pub(super) fn validate(
    original: &Tree<'_>,
    proposed: &Tree<'_>,
    before: &[Attachment],
    after: &[Attachment],
    configuration: &Configuration,
) -> Result<(), Error> {
    if before.len() != after.len() {
        return Err(Error::internal(
            0..0,
            "formatter changed comment attachment",
        ));
    }

    for (before, after) in before.iter().zip(after) {
        if before.location != after.location {
            return Err(Error::internal(
                0..0,
                "formatter changed comment attachment",
            ));
        }

        let original_token = original.token(before.token);
        let proposed_token = proposed.token(after.token);
        let original_bytes = original_token.bytes(original.source);
        let proposed_bytes = proposed_token.bytes(proposed.source);

        if original_bytes == proposed_bytes {
            continue;
        }

        let expected = standalone(proposed, after.token, configuration)
            .and_then(|columns| reindent(original, before.token, columns, configuration));

        if expected.as_deref() != Some(proposed_bytes) {
            return Err(Error::internal(
                original_token.span.start..original_token.span.end,
                "formatter changed comment text",
            ));
        }
    }

    Ok(())
}

fn standalone(tree: &Tree<'_>, index: TokenIndex, configuration: &Configuration) -> Option<usize> {
    let token = tree.token(index);

    if token.kind != TokenKind::BlockComment || !token.bytes(tree.source).contains(&b'\n') {
        return None;
    }

    let preceding = &tree.source[..token.span.start];

    let start = preceding
        .iter()
        .rposition(|byte| *byte == b'\n')
        .map_or(0, |position| position + 1);

    let leading = &preceding[start..];

    if !leading.iter().all(|byte| matches!(byte, b' ' | b'\t')) {
        return None;
    }

    let following = &tree.source[token.span.end..];

    let end = following
        .iter()
        .position(|byte| *byte == b'\n')
        .unwrap_or(following.len());

    if !following[..end]
        .iter()
        .all(|byte| matches!(byte, b' ' | b'\t' | b'\r'))
    {
        return None;
    }

    Some(indentation(leading, configuration.indent_width.get()))
}

fn indentation(bytes: &[u8], width: usize) -> usize {
    bytes.iter().fold(0, |columns, byte| {
        columns
            + if *byte == b'\t' {
                width - columns % width
            } else {
                1
            }
    })
}

fn owners(tree: &Tree<'_>, targets: &BTreeSet<usize>) -> Option<BTreeMap<usize, NodeIndex>> {
    let mut pending = targets.clone();
    let mut owners = BTreeMap::new();

    for (index, node) in tree.nodes.iter().enumerate() {
        if pending.is_empty() {
            break;
        }

        let contained: Vec<_> = pending
            .range(node.tokens.start.get()..node.tokens.end.get())
            .copied()
            .collect();

        for position in contained {
            pending.remove(&position);
            owners.insert(position, NodeIndex::new(index));
        }
    }

    pending.is_empty().then_some(owners)
}

fn identity(node: NodeIndex, correspondence: Option<&[Option<NodeIndex>]>) -> Option<usize> {
    match correspondence {
        Some(nodes) => nodes.get(node.get()).copied().flatten().map(NodeIndex::get),
        None => Some(node.get()),
    }
}

fn interior(kind: &NodeKind) -> bool {
    !matches!(
        kind,
        NodeKind::Root { .. }
            | NodeKind::Block { .. }
            | NodeKind::Function { .. }
            | NodeKind::If { .. }
            | NodeKind::Branch { .. }
            | NodeKind::Else { .. }
            | NodeKind::While { .. }
            | NodeKind::Repeat { .. }
            | NodeKind::NumericFor { .. }
            | NodeKind::GenericFor { .. }
            | NodeKind::Do { .. }
    )
}
