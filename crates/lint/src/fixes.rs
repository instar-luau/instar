use std::{io, time::Instant};

use instar_analysis::{Edit, Location, Options, error::invalid};
use instar_syntax::bindings::Bindings;

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

use crate::{Diagnostic, Kind, Level, Rule};

/// Plans safe replacements for supported findings from the exact source revision.
///
/// Unsupported findings and incomplete syntax produce no edits. Nothing is written.
///
/// # Errors
/// Returns mismatched revisions, invalid ranges, or interruption failures.
pub fn fixes<Module: Clone>(
    diagnostic: &Diagnostic<Module>,
    source: &str,
    revision: u64,
    options: &Options,
) -> io::Result<Vec<Edit<Module>>> {
    let started = Instant::now();
    options.check(started)?;
    let range = diagnostic.location.range;

    if revision != diagnostic.location.revision || source.get(range[0]..range[1]).is_none() {
        return Err(invalid("diagnostic fix source revision mismatch"));
    }

    let Kind::Rule(rule) = diagnostic.kind else {
        return Ok(Vec::new());
    };

    if diagnostic.level == Level::Allow
        || !matches!(rule, Rule::ParenthesizedConditions | Rule::SelfAssignment)
    {
        return Ok(Vec::new());
    }

    let tree = vermis::parse(source.as_bytes());

    if !tree.diagnostics.is_empty() {
        return Ok(Vec::new());
    }

    let bindings = Bindings::analyze(&tree, Some((options, started)));

    if let Some(reason) = bindings.interruption {
        return Err(reason.error());
    }

    let mut pending = vec![tree.root];

    while let Some(node) = pending.pop() {
        options.check(started)?;

        let replacements = match rule {
            Rule::ParenthesizedConditions => parentheses(&tree, node, range),
            Rule::SelfAssignment => assignment(&tree, node, range, &bindings),
            _ => Vec::new(),
        };

        if !replacements.is_empty() {
            return Ok(replacements
                .into_iter()
                .map(|(range, text)| Edit {
                    location: Location {
                        module: diagnostic.location.module.clone(),
                        revision,
                        range,
                    },
                    text,
                })
                .collect());
        }

        pending.extend(tree.children(node));
    }

    Ok(Vec::new())
}

fn parentheses(tree: &Tree<'_>, node: NodeIndex, range: [usize; 2]) -> Vec<([usize; 2], String)> {
    let (NodeKind::Branch { condition, .. }
    | NodeKind::While { condition, .. }
    | NodeKind::Repeat { condition, .. }
    | NodeKind::Conditional { condition, .. }) = &tree.node(node).kind
    else {
        return Vec::new();
    };

    let condition = tree.node(*condition);

    if !matches!(condition.kind, NodeKind::Group { .. })
        || [condition.span.start, condition.span.end] != range
        || tree.source.get(range[0]) != Some(&b'(')
        || tree.source.get(range[1].saturating_sub(1)) != Some(&b')')
    {
        return Vec::new();
    }

    vec![
        ([range[0], range[0] + 1], " ".to_owned()),
        ([range[1] - 1, range[1]], " ".to_owned()),
    ]
}

fn assignment(
    tree: &Tree<'_>,
    node: NodeIndex,
    range: [usize; 2],
    bindings: &Bindings,
) -> Vec<([usize; 2], String)> {
    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
    else {
        return Vec::new();
    };

    let ([target], [value]) = (tree.list(targets), tree.list(values)) else {
        return Vec::new();
    };

    let target_span = tree.node(target.node).span;

    if [target_span.start, target_span.end] != range
        || bindings.declaration(target.node).is_none()
        || !matches!(tree.node(target.node).kind, NodeKind::Name { .. })
        || !matches!(tree.node(value.node).kind, NodeKind::Name { .. })
        || bindings.references.get(&target.node.get()) != bindings.references.get(&value.node.get())
    {
        return Vec::new();
    }

    let span = tree.node(node).span;

    if tree.tokens.iter().any(|token| {
        matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment)
            && token.span.start < span.end
            && span.start < token.span.end
    }) {
        return Vec::new();
    }

    vec![([span.start, span.end], ";".to_owned())]
}
