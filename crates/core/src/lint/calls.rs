use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::{
    lint::{
        context::Context,
        expression::{comparison, string},
    },
    syntax::expression::{number, unwrap},
};

const RESULTS: &[&str] = &[
    "math.abs",
    "math.floor",
    "math.ceil",
    "math.sqrt",
    "math.max",
    "math.min",
    "math.sin",
    "math.cos",
    "math.tan",
    "math.log",
    "math.exp",
    "string.lower",
    "string.upper",
    "string.sub",
    "string.len",
    "string.rep",
    "string.reverse",
    "string.format",
    "table.clone",
    "table.concat",
    "table.find",
];

const ARITHMETIC: &[&str] = &[
    "math.abs",
    "math.floor",
    "math.ceil",
    "math.sqrt",
    "math.sin",
    "math.cos",
    "math.tan",
    "math.exp",
];

pub(super) fn analyze(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Call { callee, arguments }
            if context.active(&[
                "type_check_inside_call",
                "restricted_module_paths",
                "non_const_require",
                "roblox_incorrect_color3_new_bounds",
                "roblox_suspicious_udim2_new",
                "roblox_manual_fromscale_or_fromoffset",
                "loop_invariant_call",
            ]) =>
        {
            call(context, node, *callee, *arguments);
        }

        NodeKind::MethodCall {
            receiver, method, ..
        } if context.active(&["roblox_prefer_get_players"])
            && tree.text(*method) == b"GetChildren" =>
        {
            let receiver = context.bindings().value(tree, *receiver);

            if let NodeKind::MethodCall {
                receiver,
                method,
                arguments,
                ..
            } = tree.node(receiver).kind
                && tree.text(method) == b"GetService"
                && context.bindings().global(tree, receiver, "game")
                && let [argument] = arguments_list(tree, arguments).as_slice()
                && string(tree, *argument).as_deref() == Some(b"Players")
            {
                context.warn(
                    node,
                    "roblox_prefer_get_players",
                    "use Players:GetPlayers() to select players",
                );
            }
        }

        NodeKind::CallStatement { call }
            if context.active(&["ignored_pcall_result", "must_use"]) =>
        {
            if let NodeKind::Call { callee, .. } = tree.node(unwrap(tree, *call)).kind {
                let path = context.bindings().path(tree, callee);

                if context.active(&["ignored_pcall_result"])
                    && (context.bindings().global(tree, callee, "pcall")
                        || context.bindings().global(tree, callee, "xpcall"))
                {
                    context.warn(
                        *call,
                        "ignored_pcall_result",
                        "protected-call result is discarded",
                    );
                }

                if context.active(&["must_use"])
                    && RESULTS.contains(&path.as_str())
                    && context.bindings().global(tree, callee, &path)
                {
                    context.warn(
                        *call,
                        "must_use",
                        "result of a result-producing function is discarded",
                    );
                }
            }
        }

        _ => {}
    }
}

fn call(context: &mut Context<'_, '_>, node: NodeIndex, callee: NodeIndex, arguments: NodeIndex) {
    let tree = context.tree;
    let arguments = arguments_list(tree, arguments);
    let path = context.bindings().path(tree, callee);

    if context.active(&["type_check_inside_call"])
        && (context.bindings().global(tree, callee, "type")
            || context.bindings().global(tree, callee, "typeof"))
        && let [argument] = arguments.as_slice()
        && matches!(tree.node(unwrap(tree, *argument)).kind, NodeKind::Binary { operator, .. } if comparison(tree.token(operator).kind))
    {
        context.warn(
            node,
            "type_check_inside_call",
            "comparison belongs outside the type check",
        );
    }

    if context.active(&["restricted_module_paths", "non_const_require"])
        && context.bindings().global(tree, callee, "require")
    {
        if let [argument] = arguments.as_slice() {
            let value = context.bindings().value(tree, *argument);

            if let Some(literal) = string(tree, value) {
                if let Ok(path) = String::from_utf8(literal)
                    && let Some(reason) = context.settings.imports.get(&path)
                {
                    context.warn(
                        *argument,
                        "restricted_module_paths",
                        &format!("module path `{path}` is restricted: {reason}"),
                    );
                }
            } else if context.active(&["non_const_require"]) && !instance(context, value) {
                context.warn(
                    node,
                    "non_const_require",
                    "require argument is not statically known",
                );
            }
        } else {
            context.warn(
                node,
                "non_const_require",
                "require argument is not statically known",
            );
        }
    }

    if context.active(&[
        "roblox_incorrect_color3_new_bounds",
        "roblox_suspicious_udim2_new",
        "roblox_manual_fromscale_or_fromoffset",
    ]) && let NodeKind::Field { receiver, .. } = tree.node(unwrap(tree, callee)).kind
    {
        let root = context.bindings().path(tree, receiver);

        if context.symbols.globals.contains(&root) && context.bindings().global(tree, callee, &path)
        {
            if context.active(&["roblox_incorrect_color3_new_bounds"])
                && path == "Color3.new"
                && arguments.iter().any(|argument| {
                    number(tree, *argument).is_some_and(|value| !(0.0..=1.0).contains(&value))
                })
            {
                context.warn(
                    node,
                    "roblox_incorrect_color3_new_bounds",
                    "Color3.new channels use a 0-1 scale",
                );
            } else if path == "UDim2.new" {
                if arguments.len() == 2 {
                    context.warn(
                        node,
                        "roblox_suspicious_udim2_new",
                        "UDim2.new needs scale and offset components for both axes",
                    );
                } else if let [
                    horizontal_scale,
                    horizontal_offset,
                    vertical_scale,
                    vertical_offset,
                ] = arguments.as_slice()
                    && context.active(&["roblox_manual_fromscale_or_fromoffset"])
                    && ((number(tree, *horizontal_offset) == Some(0.0)
                        && number(tree, *vertical_offset) == Some(0.0))
                        || (number(tree, *horizontal_scale) == Some(0.0)
                            && number(tree, *vertical_scale) == Some(0.0)))
                {
                    context.warn(
                        node,
                        "roblox_manual_fromscale_or_fromoffset",
                        "use UDim2.fromScale or UDim2.fromOffset for zero components",
                    );
                }
            }
        }
    }

    if context.active(&["loop_invariant_call"])
        && let Some(body) = context.bindings().loops[node.get()]
        && ARITHMETIC.contains(&path.as_str())
        && context.bindings().global(tree, callee, &path)
        && let [argument] = arguments.as_slice()
        && number(tree, context.bindings().value(tree, *argument)).is_some_and(f64::is_finite)
        && context.bindings().references[unwrap(tree, *argument).get()].is_none_or(|index| {
            tree.node(context.bindings().locals[index].declaration)
                .span
                .end
                < tree.node(body).span.start
        })
    {
        context.warn(
            node,
            "loop_invariant_call",
            "deterministic numeric call has loop-invariant arguments",
        );
    }
}

fn instance(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    let tree = context.tree;

    match tree.node(unwrap(tree, node)).kind {
        NodeKind::Name { .. } => ["script", "game", "workspace"]
            .iter()
            .any(|expected| context.bindings().global(tree, node, expected)),

        NodeKind::Field { receiver, .. } => instance(context, receiver),

        NodeKind::MethodCall {
            receiver,
            method,
            arguments,
            ..
        } => {
            matches!(
                tree.text(method),
                b"GetService" | b"WaitForChild" | b"FindFirstChild"
            ) && arguments_list(tree, arguments)
                .first()
                .is_some_and(|argument| string(tree, *argument).is_some())
                && instance(context, receiver)
        }

        _ => false,
    }
}

pub(super) fn arguments_list(tree: &Tree<'_>, node: NodeIndex) -> Vec<NodeIndex> {
    match &tree.node(node).kind {
        NodeKind::Arguments { values, .. } => {
            tree.list(values).iter().map(|entry| entry.node).collect()
        }

        NodeKind::String { .. } | NodeKind::Table { .. } => vec![node],
        _ => Vec::new(),
    }
}
