use std::cmp::Ordering;

use vermis::{
    token::{Keyword, Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{
    lint::Error,
    syntax::{
        self,
        expression::{number, unwrap},
    },
};

pub(crate) fn parse(source: &str) -> Result<Tree<'_>, Error> {
    let prefix = usize::from(source.starts_with('\u{feff}')) * 3;

    syntax::parse(&source.as_bytes()[prefix..], |range, message| Error {
        range: range.start + prefix..range.end + prefix,
        message: message.unwrap_or("Syntax is incomplete.").to_owned(),
    })
}

pub(super) fn is_truthy(tree: &Tree<'_>, node: NodeIndex) -> Option<bool> {
    match &tree.node(unwrap(tree, node)).kind {
        NodeKind::Nil { .. } => Some(false),

        NodeKind::Boolean { token } => {
            Some(tree.token(*token).kind == TokenKind::Keyword(Keyword::True))
        }

        NodeKind::Number { .. }
        | NodeKind::String { .. }
        | NodeKind::Table { .. }
        | NodeKind::Function { .. } => Some(true),

        NodeKind::Unary { operator, operand }
            if tree.token(*operator).kind == TokenKind::Keyword(Keyword::Not) =>
        {
            is_truthy(tree, *operand).map(|is_true| !is_true)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } => {
            let comparison = number(tree, *left)?.partial_cmp(&number(tree, *right)?);

            match tree.token(*operator).kind {
                TokenKind::Symbol(Symbol::Equal) => Some(comparison == Some(Ordering::Equal)),
                TokenKind::Symbol(Symbol::NotEqual) => Some(comparison != Some(Ordering::Equal)),
                TokenKind::Symbol(Symbol::LessThan) => Some(comparison == Some(Ordering::Less)),

                TokenKind::Symbol(Symbol::GreaterThan) => {
                    Some(comparison == Some(Ordering::Greater))
                }

                TokenKind::Symbol(Symbol::LessThanOrEqual) => {
                    Some(matches!(comparison, Some(Ordering::Less | Ordering::Equal)))
                }

                TokenKind::Symbol(Symbol::GreaterThanOrEqual) => Some(matches!(
                    comparison,
                    Some(Ordering::Greater | Ordering::Equal)
                )),

                _ => number(tree, node).map(|_| true),
            }
        }

        _ => number(tree, node).map(|_| true),
    }
}

pub(super) fn body(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    if let NodeKind::Else { body, .. } = tree.node(node).kind {
        body
    } else {
        node
    }
}

pub(super) fn single(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Block { statements } = &tree.node(body(tree, node)).kind else {
        return None;
    };

    let [statement] = tree.list(statements) else {
        return None;
    };

    Some(statement.node)
}

pub(super) fn is_same(tree: &Tree<'_>, left: NodeIndex, right: NodeIndex) -> bool {
    let tokens = |node: NodeIndex| {
        let range = &tree.node(node).tokens;

        tree.tokens[range.start.get()..range.end.get()]
            .iter()
            .filter(|token| {
                !matches!(
                    token.kind,
                    TokenKind::Whitespace | TokenKind::Comment | TokenKind::BlockComment
                )
            })
            .map(|token| (token.kind, token.span.bytes(tree.source)))
    };

    tokens(left).eq(tokens(right))
}
