use std::collections::{BTreeMap, BTreeSet};

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use super::syntax::substantive;

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
enum Location {
    Global,

    Interior {
        node: usize,
        gap: usize,
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

#[derive(Eq, PartialEq)]
pub(super) struct Attachment<'source> {
    location: Location,
    text: &'source [u8],
}

struct Comment {
    position: usize,
    adjacent: Option<usize>,
    inline: bool,
    global: bool,
}

pub(super) fn attached<'source>(
    tree: &Tree<'source>,
    correspondence: Option<&[Option<NodeIndex>]>,
    ordering: bool,
) -> Option<Vec<Attachment<'source>>> {
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
        let location = if comment.global {
            Location::Global
        } else {
            let owner = *owners.get(&comment.position)?;

            if interior(&tree.node(owner).kind) {
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
            } else if let Some(adjacent) = comment.adjacent {
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

                Location::Adjacent {
                    node,
                    gap,
                    token,
                    inline: comment.inline,
                }
            } else {
                Location::Detached {
                    inline: comment.inline,
                }
            }
        };

        result.push(Attachment {
            location,
            text: tree.tokens[comment.position].bytes(tree.source),
        });
    }

    if ordering {
        result.sort_by_key(|attachment| attachment.location);
    }

    Some(result)
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
