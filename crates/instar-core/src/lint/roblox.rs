use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, Tree},
};

use super::{Context, Finding};

fn numeric_literal(tree: &Tree<'_>, node: NodeIndex) -> Option<f64> {
    if matches!(tree.node(node).kind, NodeKind::Number { .. }) {
        return std::str::from_utf8(tree.text(node)).ok()?.parse().ok();
    }

    if let NodeKind::Unary { operator, operand } = &tree.node(node).kind
        && tree.token(*operator).kind == TokenKind::Symbol(Symbol::Subtract)
    {
        return numeric_literal(tree, *operand).map(|number| -number);
    }

    None
}

fn is_color3_new(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let NodeKind::Call { callee, .. } = &tree.node(node).kind else {
        return false;
    };

    let NodeKind::Field { receiver, name, .. } = &tree.node(*callee).kind else {
        return false;
    };

    matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
        && tree.text(*receiver) == b"Color3"
        && tree.text(*name) == b"new"
}

fn is_udim2_new(tree: &Tree<'_>, node: NodeIndex) -> Option<NodeIndex> {
    let NodeKind::Call { callee, arguments } = &tree.node(node).kind else {
        return None;
    };

    let NodeKind::Field { receiver, name, .. } = &tree.node(*callee).kind else {
        return None;
    };

    (matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
        && tree.text(*receiver) == b"UDim2"
        && tree.text(*name) == b"new")
        .then_some(*arguments)
}

pub(super) fn check(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    if is_color3_new(tree, node)
        && context.enabled("roblox_incorrect_color3_new_bounds")
        && !super::suspicious::has_local(tree, b"Color3", tree.node(node).span.start, ancestors)
        && let NodeKind::Call { arguments, .. } = &tree.node(node).kind
        && let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind
        && tree.list(values).iter().any(|value| {
            numeric_literal(tree, value.node).is_some_and(|channel| !(0.0..=1.0).contains(&channel))
        })
    {
        context.emit(
            findings,
            "roblox_incorrect_color3_new_bounds",
            tree.node(node).span,
            "Color3.new channels use a 0–1 scale",
        );
    }

    if !super::suspicious::has_local(tree, b"UDim2", tree.node(node).span.start, ancestors)
        && let Some(arguments) = is_udim2_new(tree, node)
    {
        let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind else {
            return;
        };

        let values = tree.list(values);
        let count = values.len();

        if count == 2 && context.enabled("roblox_suspicious_udim2_new") {
            context.emit(
                findings,
                "roblox_suspicious_udim2_new",
                tree.node(node).span,
                "UDim2.new expects scale and offset components for both axes",
            );
        } else if count == 4 && context.enabled("roblox_manual_fromscale_or_fromoffset") {
            let scale = numeric_literal(tree, values[1].node).is_some_and(|value| value == 0.0)
                && numeric_literal(tree, values[3].node).is_some_and(|value| value == 0.0);

            let offset = numeric_literal(tree, values[0].node).is_some_and(|value| value == 0.0)
                && numeric_literal(tree, values[2].node).is_some_and(|value| value == 0.0);

            if scale || offset {
                context.emit(
                    findings,
                    "roblox_manual_fromscale_or_fromoffset",
                    tree.node(node).span,
                    if scale {
                        "use UDim2.fromScale when both offsets are zero"
                    } else {
                        "use UDim2.fromOffset when both scales are zero"
                    },
                );
            }
        }
    }

    if context.enabled("roblox_prefer_get_players")
        && let NodeKind::MethodCall {
            receiver, method, ..
        } = &tree.node(node).kind
        && tree.text(*method) == b"GetChildren"
        && !super::suspicious::has_local(tree, b"game", tree.node(node).span.start, ancestors)
        && (is_confirmed_players(tree, *receiver)
            || (matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
                && players_binding(tree, *receiver, ancestors)))
    {
        context.emit(
            findings,
            "roblox_prefer_get_players",
            tree.node(node).span,
            "use Players:GetPlayers() to select players",
        );
    }
}

fn is_confirmed_players(tree: &Tree<'_>, node: NodeIndex) -> bool {
    let NodeKind::MethodCall {
        receiver,
        method,
        arguments,
        ..
    } = &tree.node(node).kind
    else {
        return false;
    };

    if tree.text(*method) != b"GetService"
        || !matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
        || tree.text(*receiver) != b"game"
    {
        return false;
    }

    let NodeKind::Arguments { values, .. } = &tree.node(*arguments).kind else {
        return false;
    };

    let values = tree.list(values);

    values.len() == 1
        && matches!(tree.node(values[0].node).kind, NodeKind::String { .. })
        && matches!(tree.text(values[0].node), b"\"Players\"" | b"'Players'")
}

fn players_binding(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    for &ancestor in ancestors.iter().rev() {
        let NodeKind::Block { statements } = &tree.node(ancestor).kind else {
            continue;
        };

        let mut declared = None;

        for statement in tree
            .list(statements)
            .iter()
            .filter(|statement| tree.node(statement.node).span.end <= tree.node(node).span.start)
        {
            match &tree.node(statement.node).kind {
                NodeKind::Local {
                    bindings, values, ..
                }
                | NodeKind::Constant {
                    bindings, values, ..
                } => {
                    for (binding, value) in tree.list(bindings).iter().zip(tree.list(values)) {
                        if let NodeKind::Binding { name, .. } = &tree.node(binding.node).kind
                            && tree.text(*name) == tree.text(node)
                        {
                            declared = Some(is_confirmed_players(tree, value.node));
                        }
                    }
                }

                NodeKind::Assignment { targets, .. } if declared.is_some() => {
                    if tree.list(targets).iter().any(|target| {
                        matches!(tree.node(target.node).kind, NodeKind::Name { .. })
                            && tree.text(target.node) == tree.text(node)
                    }) {
                        declared = Some(false);
                    }
                }

                NodeKind::CompoundAssignment { target, .. }
                    if declared.is_some()
                        && matches!(tree.node(*target).kind, NodeKind::Name { .. })
                        && tree.text(*target) == tree.text(node) =>
                {
                    declared = Some(false);
                }

                _ => {}
            }
        }

        if let Some(confirmed) = declared {
            return confirmed;
        }
    }

    false
}
