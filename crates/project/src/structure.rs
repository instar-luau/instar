//! Syntax folding and nested source selections.

use std::{cmp::Reverse, io};

use instar_shared::{Options, error::invalid};

use instar_syntax::{
    token::TokenKind,
    tree::{Node, NodeKind, Tree},
};

/// A foldable source region with byte offsets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fold {
    /// Source range, ending before a syntax closing delimiter when available.
    pub range: [usize; 2],

    /// Whether this is a comment rather than executable or type syntax.
    pub comment: bool,
}

pub(crate) fn folds(
    source: &instar_shared::SourceSnapshot,
    settings: &instar_syntax::SyntaxSettings,
    options: &Options,
) -> io::Result<Vec<Fold>> {
    let syntax = instar_syntax::parse(source, settings, options);
    let tree = syntax.tree.syntax();
    let mut folds = Vec::new();

    for node in &tree.nodes {
        options.check()?;

        if let Some(fold) = fold(tree, node) {
            folds.push(fold);
        }
    }

    let mut previous_comment: Option<usize> = None;

    for token in &tree.tokens {
        options.check()?;

        if token.kind == TokenKind::Comment {
            if let Some(index) = previous_comment
                && source
                    .text
                    .get(folds[index].range[1]..token.span.start)
                    .is_some_and(|gap| gap.trim().is_empty())
            {
                folds[index].range[1] = token.span.end;
                continue;
            }

            previous_comment = Some(folds.len());

            folds.push(Fold {
                range: [token.span.start, token.span.end],
                comment: true,
            });
        } else if matches!(
            token.kind,
            TokenKind::BlockComment
                | TokenKind::MalformedComment
                | TokenKind::RawString
                | TokenKind::QuotedString
                | TokenKind::InterpolatedStringStart
                | TokenKind::InterpolatedStringMiddle
                | TokenKind::InterpolatedStringEnd
                | TokenKind::InterpolatedStringSimple
                | TokenKind::MalformedString
        ) {
            folds.push(Fold {
                range: [token.span.start, token.span.end],
                comment: matches!(
                    token.kind,
                    TokenKind::BlockComment | TokenKind::MalformedComment
                ),
            });
        }
    }

    folds.retain(|fold| {
        source
            .text
            .get(fold.range[0]..fold.range[1])
            .is_some_and(|text| text.contains(['\r', '\n']))
    });

    folds.sort_by_key(|fold| (fold.range[0], Reverse(fold.range[1])));
    folds.dedup();
    options.check()?;

    Ok(folds)
}

pub(crate) fn selections(
    source: &instar_shared::SourceSnapshot,
    settings: &instar_syntax::SyntaxSettings,
    offsets: &[usize],
    options: &Options,
) -> io::Result<Vec<Vec<[usize; 2]>>> {
    let syntax = instar_syntax::parse(source, settings, options);
    let tree = syntax.tree.syntax();
    let mut selections = Vec::new();

    for &offset in offsets {
        options.check()?;

        if source.text.get(..offset).is_none() {
            return Err(invalid("invalid selection position"));
        }

        let mut ranges = vec![[0, source.text.len()]];

        for span in tree
            .nodes
            .iter()
            .map(|node| node.span)
            .chain(tree.tokens.iter().map(|token| token.span))
        {
            options.check()?;

            if span.start <= offset
                && offset <= span.end
                && span.start < span.end
                && source.text.get(span.start..span.end).is_some()
            {
                ranges.push([span.start, span.end]);
            }
        }

        ranges.sort_unstable_by_key(|range| (range[1] - range[0], range[0]));
        ranges.dedup();
        let mut nested: Vec<[usize; 2]> = Vec::new();

        for range in ranges {
            options.check()?;

            if nested
                .last()
                .is_none_or(|previous| range[0] <= previous[0] && previous[1] <= range[1])
            {
                nested.push(range);
            }
        }

        selections.push(nested);
    }

    options.check()?;

    Ok(selections)
}

fn fold(tree: &Tree<'_>, node: &Node) -> Option<Fold> {
    let closing = match node.kind {
        NodeKind::Function {
            body: Some(_), end, ..
        }
        | NodeKind::If { end, .. }
        | NodeKind::While { end, .. }
        | NodeKind::GenericFor { end, .. }
        | NodeKind::Do { end, .. } => end,

        NodeKind::NumericFor { end_keyword, .. } => end_keyword,
        NodeKind::Repeat { until, .. } => until,

        NodeKind::Table { closing, .. }
        | NodeKind::TypeTable { closing, .. }
        | NodeKind::Arguments { closing, .. }
        | NodeKind::Parameters { closing, .. } => closing,

        NodeKind::Interpolation { .. } => None,
        _ => return None,
    };

    Some(Fold {
        range: [
            node.span.start,
            closing.map_or(node.span.end, |closing| tree.token(closing).span.start),
        ],
        comment: false,
    })
}
