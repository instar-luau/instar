use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::{
    lint::{
        context::Context,
        expression::{is_comparison, string},
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

pub(super) fn check(context: &mut Context<'_, '_>, node: NodeIndex) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Call { callee, arguments }
            if context.is_active(&[
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
        } if context.is_active(&["roblox_prefer_get_players"])
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
                && context.bindings().is_global(tree, receiver, "game")
                && let [argument] = arguments_list(tree, arguments).as_slice()
                && string(tree, *argument).as_deref() == Some(b"Players")
            {
                context.emit(
                    node,
                    "roblox_prefer_get_players",
                    "This call uses `GetChildren` on the `Players` service; use `Players:GetPlayers()` to select players.",
                );
            }
        }

        NodeKind::CallStatement { call }
            if context.is_active(&["ignored_pcall_result", "must_use"]) =>
        {
            if let NodeKind::Call { callee, .. } = tree.node(unwrap(tree, *call)).kind {
                let path = context.bindings().path(tree, callee);

                if context.is_active(&["ignored_pcall_result"])
                    && (context.bindings().is_global(tree, callee, "pcall")
                        || context.bindings().is_global(tree, callee, "xpcall"))
                {
                    context.emit(
                        *call,
                        "ignored_pcall_result",
                        "Protected-call result is discarded.",
                    );
                }

                if context.is_active(&["must_use"])
                    && RESULTS.contains(&path.as_str())
                    && context.bindings().is_global(tree, callee, &path)
                {
                    context.emit(
                        *call,
                        "must_use",
                        &format!("The result of `{path}` is discarded."),
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

    if context.is_active(&["type_check_inside_call"])
        && (context.bindings().is_global(tree, callee, "type")
            || context.bindings().is_global(tree, callee, "typeof"))
        && let [argument] = arguments.as_slice()
        && matches!(tree.node(unwrap(tree, *argument)).kind, NodeKind::Binary { operator, .. } if is_comparison(tree.token(operator).kind))
    {
        context.emit(
            node,
            "type_check_inside_call",
            "A comparison is passed to `type` or `typeof`; check whether the comparison belongs outside the call.",
        );
    }

    if context.is_active(&["restricted_module_paths", "non_const_require"])
        && context.bindings().is_global(tree, callee, "require")
    {
        if let [argument] = arguments.as_slice() {
            let value = context.bindings().value(tree, *argument);

            if let Some(literal) = string(tree, value) {
                if let Ok(path) = String::from_utf8(literal)
                    && let Some(reason) = context.settings.imports.get(&path)
                {
                    context.emit(
                        *argument,
                        "restricted_module_paths",
                        &format!("Module path `{path}` is restricted.\n{reason}"),
                    );
                }
            } else if context.is_active(&["non_const_require"]) && !is_instance(context, value) {
                context.emit(
                    node,
                    "non_const_require",
                    "The `require` argument is not statically known.",
                );
            }
        } else {
            context.emit(
                node,
                "non_const_require",
                "This `require` call does not have exactly one argument.",
            );
        }
    }

    if context.is_active(&[
        "roblox_incorrect_color3_new_bounds",
        "roblox_suspicious_udim2_new",
        "roblox_manual_fromscale_or_fromoffset",
    ]) && let NodeKind::Field { receiver, .. } = tree.node(unwrap(tree, callee)).kind
    {
        let root = context.bindings().path(tree, receiver);

        if context.symbols.globals.contains(&root)
            && context.bindings().is_global(tree, callee, &path)
        {
            if context.is_active(&["roblox_incorrect_color3_new_bounds"])
                && path == "Color3.new"
                && arguments.iter().any(|argument| {
                    number(tree, *argument).is_some_and(|value| !(0.0..=1.0).contains(&value))
                })
            {
                context.emit(
                    node,
                    "roblox_incorrect_color3_new_bounds",
                    "A `Color3.new` argument is outside the range from 0 to 1.",
                );
            } else if path == "UDim2.new" {
                if arguments.len() == 2 {
                    context.emit(
                        node,
                        "roblox_suspicious_udim2_new",
                        "This `UDim2.new` call has two arguments; check the constructor overload.",
                    );
                } else if let [
                    horizontal_scale,
                    horizontal_offset,
                    vertical_scale,
                    vertical_offset,
                ] = arguments.as_slice()
                    && context.is_active(&["roblox_manual_fromscale_or_fromoffset"])
                    && ((number(tree, *horizontal_offset) == Some(0.0)
                        && number(tree, *vertical_offset) == Some(0.0))
                        || (number(tree, *horizontal_scale) == Some(0.0)
                            && number(tree, *vertical_scale) == Some(0.0)))
                {
                    context.emit(
                        node,
                        "roblox_manual_fromscale_or_fromoffset",
                        "This `UDim2.new` call has zero scale or offset components; consider `UDim2.fromScale` or `UDim2.fromOffset`.",
                    );
                }
            }
        }
    }

    if context.is_active(&["loop_invariant_call"])
        && let Some(body) = context.bindings().loops[node.get()]
        && ARITHMETIC.contains(&path.as_str())
        && context.bindings().is_global(tree, callee, &path)
        && let [argument] = arguments.as_slice()
        && number(tree, context.bindings().value(tree, *argument)).is_some_and(f64::is_finite)
        && context.bindings().references[unwrap(tree, *argument).get()].is_none_or(|index| {
            tree.node(context.bindings().locals[index].declaration)
                .span
                .end
                < tree.node(body).span.start
        })
    {
        context.emit(
            node,
            "loop_invariant_call",
            "This numeric call has a loop-invariant argument.",
        );
    }
}

fn is_instance(context: &Context<'_, '_>, node: NodeIndex) -> bool {
    let tree = context.tree;

    match tree.node(unwrap(tree, node)).kind {
        NodeKind::Name { .. } => ["script", "game", "workspace"]
            .iter()
            .any(|expected| context.bindings().is_global(tree, node, expected)),

        NodeKind::Field { receiver, .. } => is_instance(context, receiver),

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
                && is_instance(context, receiver)
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
