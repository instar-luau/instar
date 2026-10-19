use std::{mem, ops::Range};

use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use super::comments;

use crate::{
    format::{
        Error,
        directives::{Directives, Kind},
        imports::{self, Imports},
    },
    settings::format::Configuration,
    syntax::literal,
};

struct Comparison<'tree, 'source> {
    original: &'tree Tree<'source>,
    proposed: &'tree Tree<'source>,
    before: Option<&'tree Imports>,
    after: Option<Imports>,
    original_directives: &'tree Directives,
    proposed_directives: &'tree Directives,
    nodes: Vec<Option<NodeIndex>>,
}

impl Comparison<'_, '_> {
    fn matches(&mut self, original: NodeIndex, proposed: NodeIndex) -> Option<bool> {
        let left = self.original.node(original);
        let right = self.proposed.node(proposed);

        if mem::discriminant(&left.kind) != mem::discriminant(&right.kind) {
            return Some(false);
        }

        self.nodes[original.get()] = Some(proposed);

        if let (NodeKind::Block { statements: left }, NodeKind::Block { statements: right }) =
            (&left.kind, &right.kind)
        {
            let left = statements(self.original, left, self.before, self.original_directives);

            let right = statements(
                self.proposed,
                right,
                self.after.as_ref(),
                self.proposed_directives,
            );

            if left.len() != right.len() {
                return Some(false);
            }

            for (original, proposed) in left.into_iter().zip(right) {
                if !self.matches(original, proposed)? {
                    return Some(false);
                }
            }

            return Some(true);
        }

        let left_children = self.original.children(original);
        let right_children = self.proposed.children(proposed);

        if left_children.len() != right_children.len() {
            return Some(false);
        }

        let mut left_position = left.tokens.start.get();
        let mut right_position = right.tokens.start.get();

        for (original, proposed) in left_children.into_iter().zip(right_children) {
            let left_child = self.original.node(original);
            let right_child = self.proposed.node(proposed);

            if left_child.tokens.start.get() < left_position
                || right_child.tokens.start.get() < right_position
            {
                return None;
            }

            if !equivalent(
                self.original,
                left_position..left_child.tokens.start.get(),
                self.proposed,
                right_position..right_child.tokens.start.get(),
            )? || !self.matches(original, proposed)?
            {
                return Some(false);
            }

            left_position = left_child.tokens.end.get();
            right_position = right_child.tokens.end.get();
        }

        equivalent(
            self.original,
            left_position..left.tokens.end.get(),
            self.proposed,
            right_position..right.tokens.end.get(),
        )
    }
}

pub(crate) fn validate(
    original: &Tree<'_>,
    proposed: &Tree<'_>,
    configuration: &Configuration,
    imports: Option<&Imports>,
    original_directives: &Directives,
    proposed_directives: &Directives,
) -> Result<(), Error> {
    let mut comparison = Comparison {
        original,
        proposed,
        before: imports,
        after: imports.map(|_| Imports::new(proposed)),
        original_directives,
        proposed_directives,
        nodes: vec![None; original.nodes.len()],
    };

    match comparison.matches(original.root, proposed.root) {
        Some(true) => {}

        Some(false) => {
            return Err(Error::internal(
                0..0,
                "formatter changed syntax or literal values",
            ));
        }

        None => {
            return Err(Error::internal(
                0..0,
                "cannot validate syntax or literal values",
            ));
        }
    }

    let before = comments::attached(
        original,
        Some(&comparison.nodes),
        imports.is_some(),
        original_directives,
    )
    .ok_or_else(|| Error::internal(0..0, "cannot validate original comment attachment"))?;

    let after = comments::attached(proposed, None, imports.is_some(), proposed_directives)
        .ok_or_else(|| Error::internal(0..0, "cannot validate proposed comment attachment"))?;

    comments::validate(original, proposed, &before, &after, configuration)?;

    if original_directives.policies.len() != proposed_directives.policies.len() {
        return Err(Error::internal(0..0, "formatter changed directives"));
    }

    for (before, after) in original_directives
        .policies
        .iter()
        .zip(&proposed_directives.policies)
    {
        let preserved = before.kind == after.kind
            && match before.kind {
                Kind::Verbatim | Kind::Remainder => original
                    .source
                    .get(before.scope.clone())
                    .is_some_and(|source| Some(source) == proposed.source.get(after.scope.clone())),

                Kind::Global => true,

                Kind::Attached => {
                    equivalent(
                        original,
                        tokens(original, &before.scope),
                        proposed,
                        tokens(proposed, &after.scope),
                    ) == Some(true)
                }
            };

        if !preserved {
            return Err(Error::internal(
                0..0,
                "formatter changed protected source or directive scopes",
            ));
        }
    }

    Ok(())
}

pub(super) fn substantive(kind: TokenKind) -> bool {
    !matches!(
        kind,
        TokenKind::Whitespace
            | TokenKind::Comment
            | TokenKind::BlockComment
            | TokenKind::EndOfFile
            | TokenKind::Symbol(Symbol::Comma | Symbol::Semicolon)
    )
}

fn equivalent(
    original: &Tree<'_>,
    left: Range<usize>,
    proposed: &Tree<'_>,
    right: Range<usize>,
) -> Option<bool> {
    let mut left = original
        .tokens
        .get(left)?
        .iter()
        .filter(|token| substantive(token.kind));

    let mut right = proposed
        .tokens
        .get(right)?
        .iter()
        .filter(|token| substantive(token.kind));

    loop {
        match (left.next(), right.next()) {
            (None, None) => return Some(true),

            (Some(left), Some(right)) => {
                let equal = if matches!(left.kind, TokenKind::QuotedString | TokenKind::RawString)
                    && matches!(right.kind, TokenKind::QuotedString | TokenKind::RawString)
                {
                    literal::decode(left.bytes(original.source))?
                        == literal::decode(right.bytes(proposed.source))?
                } else {
                    left.kind == right.kind
                        && left.bytes(original.source) == right.bytes(proposed.source)
                };

                if !equal {
                    return Some(false);
                }
            }

            _ => return Some(false),
        }
    }
}

fn statements(
    tree: &Tree<'_>,
    statements: &NodeList,
    imports: Option<&Imports>,
    directives: &Directives,
) -> Vec<NodeIndex> {
    let entries = tree.list(statements);

    let Some(imports) = imports else {
        return entries.iter().map(|entry| entry.node).collect();
    };

    let mut statements: Vec<_> = entries
        .iter()
        .map(|entry| (entry.node, imports.require(tree, entry.node, directives)))
        .collect();

    for range in imports::runs(tree, statements.iter().map(|(_, require)| require.as_ref())) {
        statements[range].sort_by_key(|(_, require)| {
            tree.text(
                require
                    .as_ref()
                    .expect("internal error: missing require in import run")
                    .binding,
            )
        });
    }

    statements.into_iter().map(|(node, _)| node).collect()
}

fn tokens(tree: &Tree<'_>, scope: &Range<usize>) -> Range<usize> {
    let start = tree
        .tokens
        .partition_point(|token| token.span.end <= scope.start);

    let end = tree
        .tokens
        .partition_point(|token| token.span.start < scope.end);

    start..end
}
