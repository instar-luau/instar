use vermis::{
    token::{Keyword, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{Context, Finding};

fn score(tree: &Tree<'_>, node: NodeIndex, root: bool) -> usize {
    if !root
        && matches!(&tree.node(node).kind, NodeKind::Function { prefix, body: Some(_), .. }
        if prefix.is_none_or(|prefix| tree.token(prefix).bytes(tree.source) != b"type"))
    {
        return 0;
    }

    let branch = match &tree.node(node).kind {
        NodeKind::Branch { .. }
        | NodeKind::While { .. }
        | NodeKind::Repeat { .. }
        | NodeKind::NumericFor { .. }
        | NodeKind::GenericFor { .. }
        | NodeKind::Conditional { .. } => 1,

        NodeKind::Binary { operator, .. } => usize::from(matches!(
            tree.token(*operator).kind,
            TokenKind::Keyword(Keyword::And | Keyword::Or)
        )),

        _ => 0,
    };

    branch
        + tree
            .children(node)
            .into_iter()
            .map(|child| score(tree, child, false))
            .sum::<usize>()
}

pub(super) fn check(
    node: NodeIndex,
    _ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    if !context.enabled("high_cyclomatic_complexity")
        || !matches!(&tree.node(node).kind, NodeKind::Function { prefix, body: Some(_), .. }
            if prefix.is_none_or(|prefix| tree.token(prefix).bytes(tree.source) != b"type"))
    {
        return;
    }

    let complexity = score(tree, node, true) + 1;

    let maximum = context
        .config
        .high_cyclomatic_complexity
        .maximum_complexity();

    if complexity > maximum {
        context.emit(
            findings,
            "high_cyclomatic_complexity",
            tree.node(node).span,
            format!("function complexity {complexity} exceeds maximum {maximum}"),
        );
    }
}
