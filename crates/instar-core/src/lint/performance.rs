use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{Context, Finding};

fn literal_argument(tree: &Tree<'_>, arguments: NodeIndex) -> bool {
    let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind else {
        return false;
    };

    let values = tree.list(values);

    values.len() == 1 && matches!(tree.node(values[0].node).kind, NodeKind::String { .. })
}

fn repeated_in_loop(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    for &ancestor in ancestors.iter().rev() {
        if matches!(&tree.node(ancestor).kind, NodeKind::Function { prefix, body: Some(_), .. }
            if prefix.is_none_or(|prefix| tree.token(prefix).bytes(tree.source) != b"type"))
        {
            return false;
        }

        let (NodeKind::While { body, .. }
        | NodeKind::Repeat { body, .. }
        | NodeKind::NumericFor { body, .. }
        | NodeKind::GenericFor { body, .. }) = &tree.node(ancestor).kind
        else {
            continue;
        };

        let body = tree.node(*body).span;
        let span = tree.node(node).span;

        if body.start <= span.start && span.end <= body.end {
            return true;
        }
    }

    false
}

fn manual_clone(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let NodeKind::GenericFor {
        bindings,
        values,
        body,
        ..
    } = &tree.node(node).kind
    else {
        return false;
    };

    let bindings = tree.list(bindings);
    let values = tree.list(values);

    if bindings.len() != 2 || values.len() != 1 {
        return false;
    }

    let (NodeKind::Binding { name: key, .. }, NodeKind::Binding { name: value, .. }) = (
        &tree.node(bindings[0].node).kind,
        &tree.node(bindings[1].node).kind,
    ) else {
        return false;
    };

    let NodeKind::Call { callee, arguments } = &tree.node(values[0].node).kind else {
        return false;
    };

    if !matches!(tree.node(*callee).kind, NodeKind::Name { .. }) || tree.text(*callee) != b"pairs" {
        return false;
    }

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return false;
    };

    let values = tree.list(values);

    if values.len() != 1 || !matches!(tree.node(values[0].node).kind, NodeKind::Name { .. }) {
        return false;
    }

    let source = values[0].node;

    let NodeKind::Block { statements } = &tree.node(*body).kind else {
        return false;
    };

    let statements = tree.list(statements);

    if statements.len() != 1 {
        return false;
    }

    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(statements[0].node).kind
    else {
        return false;
    };

    let targets = tree.list(targets);
    let values = tree.list(values);

    if targets.len() != 1 || values.len() != 1 {
        return false;
    }

    let NodeKind::Index {
        receiver,
        key: assigned_key,
        ..
    } = &tree.node(targets[0].node).kind
    else {
        return false;
    };

    let item = values[0].node;

    matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
        && tree.text(*receiver) != tree.text(source)
        && tree.text(*assigned_key) == tree.text(*key)
        && matches!(tree.node(item).kind, NodeKind::Name { .. })
        && tree.text(item) == tree.text(*value)
}

fn accumulated_concat(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    let NodeKind::Binary { left, operator, .. } = &tree.node(node).kind else {
        return false;
    };

    if tree.token(*operator).kind != TokenKind::Symbol(Symbol::Concatenate)
        || !matches!(tree.node(*left).kind, NodeKind::Name { .. })
    {
        return false;
    }

    ancestors.iter().rev().any(|&ancestor| {
        let (target, value) = match &tree.node(ancestor).kind {
            NodeKind::Assignment {
                targets, values, ..
            } => {
                let (Some(target), Some(value)) =
                    (tree.list(targets).first(), tree.list(values).first())
                else {
                    return false;
                };

                (target.node, value.node)
            }

            NodeKind::CompoundAssignment { target, value, .. } => (*target, *value),
            _ => return false,
        };

        tree.node(value).span.start <= tree.node(node).span.start
            && tree.node(value).span.end >= tree.node(node).span.end
            && matches!(tree.node(target).kind, NodeKind::Name { .. })
            && tree.text(target) == tree.text(*left)
    })
}

pub(super) fn check(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;
    let in_loop = repeated_in_loop(tree, node, ancestors);

    if in_loop {
        match &tree.node(node).kind {
            NodeKind::Call { callee, arguments }
                if context.enabled("loop_invariant_call")
                    && matches!(tree.node(*callee).kind, NodeKind::Name { .. })
                    && tree.text(*callee) == b"require"
                    && !super::suspicious::has_local(
                        tree,
                        b"require",
                        tree.node(node).span.start,
                        ancestors,
                    )
                    && literal_argument(tree, *arguments) =>
            {
                context.emit(
                    findings,
                    "loop_invariant_call",
                    tree.node(node).span,
                    "literal require call repeated inside a loop",
                );
            }

            NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } if context.enabled("loop_invariant_call")
                && matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
                && tree.text(*receiver) == b"game"
                && tree.text(*method) == b"GetService"
                && !super::suspicious::has_local(
                    tree,
                    b"game",
                    tree.node(node).span.start,
                    ancestors,
                )
                && literal_argument(tree, *arguments) =>
            {
                context.emit(
                    findings,
                    "loop_invariant_call",
                    tree.node(node).span,
                    "GetService call repeated inside a loop",
                );
            }

            _ => {}
        }

        if context.enabled("string_concat_in_loop") && accumulated_concat(tree, node, ancestors) {
            context.emit(
                findings,
                "string_concat_in_loop",
                tree.node(node).span,
                "repeated concatenation grows a string quadratically",
            );
        }
    }

    if matches!(tree.node(node).kind, NodeKind::GenericFor { .. })
        && context.enabled("manual_table_clone")
        && !super::suspicious::has_local(tree, b"pairs", tree.node(node).span.start, ancestors)
        && manual_clone(tree, node)
    {
        context.emit(
            findings,
            "manual_table_clone",
            tree.node(node).span,
            "table-copy loop may be replaced with table.clone",
        );
    }
}
