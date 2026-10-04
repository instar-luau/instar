use vermis::{
    token::{Symbol, TokenKind},
    tree::{NodeIndex, NodeKind, NodeList, Tree},
};

use super::{Context, Finding};

pub(super) fn check(
    node: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    match &tree.node(node).kind {
        NodeKind::Block { statements } => almost_swapped(statements, context, findings),

        NodeKind::NumericFor {
            binding,
            step,
            body,
            ..
        } => {
            if step.is_some_and(|step| super::suspicious::is_zero(tree, step)) {
                context.emit(
                    findings,
                    "zero_step_loop",
                    tree.node(node).span,
                    "numeric loop step is zero",
                );
            }

            if context.config.unused_variable.loop_variables() {
                unused_binding(*binding, *body, context, findings);
            }
        }

        NodeKind::GenericFor { bindings, body, .. } => {
            if context.config.unused_variable.loop_variables() {
                for binding in tree.list(bindings) {
                    unused_binding(binding.node, *body, context, findings);
                }
            }
        }

        NodeKind::Function {
            parameters,
            body: Some(body),
            ..
        } => {
            if context.config.unused_variable.parameters()
                && let NodeKind::Parameters { parameters, .. } = &tree.node(*parameters).kind
            {
                for binding in tree.list(parameters) {
                    unused_binding(binding.node, *body, context, findings);
                }
            }
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } if matches!(
            tree.token(*operator).kind,
            TokenKind::Symbol(Symbol::Equal | Symbol::NotEqual)
        ) =>
        {
            if is_nan(tree, *left) || is_nan(tree, *right) {
                context.emit(
                    findings,
                    "compare_nan",
                    tree.node(node).span,
                    "comparison with literal NaN has a fixed result",
                );
            }

            if is_fresh_table(tree, *left) || is_fresh_table(tree, *right) {
                context.emit(
                    findings,
                    "constant_table_comparison",
                    tree.node(node).span,
                    "fresh table is compared by identity",
                );
            }
        }

        NodeKind::Call { callee, arguments } => {
            check_call(node, *callee, *arguments, ancestors, context, findings);
        }

        NodeKind::Branch { condition, .. }
        | NodeKind::While { condition, .. }
        | NodeKind::Repeat { condition, .. }
        | NodeKind::Conditional { condition, .. } => check_condition(*condition, context, findings),

        NodeKind::CallStatement { call } if is_pure_call(tree, *call, ancestors) => {
            context.emit(
                findings,
                "must_use",
                tree.node(*call).span,
                "result of a pure function is discarded",
            );
        }

        _ => {}
    }

    if matches!(tree.node(node).kind, NodeKind::String { .. }) && bad_escape(tree.text(node)) {
        context.emit(
            findings,
            "bad_string_escape",
            tree.node(node).span,
            "string contains an undefined escape",
        );
    }
}

fn check_call(
    node: NodeIndex,
    callee: NodeIndex,
    arguments: NodeIndex,
    ancestors: &[NodeIndex],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    if matches!(tree.text(callee), b"type" | b"typeof")
        && matches!(tree.node(callee).kind, NodeKind::Name { .. })
        && let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind
        && tree.list(values).len() == 1
        && is_comparison(tree, tree.list(values)[0].node)
        && !super::suspicious::has_local(
            tree,
            tree.text(callee),
            tree.node(node).span.start,
            ancestors,
        )
    {
        context.emit(
            findings,
            "type_check_inside_call",
            tree.node(node).span,
            "comparison belongs outside the type check",
        );
    }

    if context.enabled("mismatched_arg_count")
        && matches!(tree.node(callee).kind, NodeKind::Name { .. })
        && let Some((minimum, variadic)) = context.writes.arity(tree.node(callee).span.start)
    {
        let (actual, expands) = call_arity(tree, arguments);

        if actual > minimum && !variadic || actual < minimum && !expands {
            context.emit(
                findings,
                "mismatched_arg_count",
                tree.node(node).span,
                "call argument count differs from the local function declaration",
            );
        }
    }
}

fn condition(tree: &Tree<'_>, node: NodeIndex) -> NodeIndex {
    match &tree.node(node).kind {
        NodeKind::Group { expression, .. } => condition(tree, *expression),
        _ => node,
    }
}

fn check_condition(node: NodeIndex, context: &Context<'_>, findings: &mut Vec<Finding>) {
    let tree = context.tree;
    let node = condition(tree, node);

    if matches!(
        tree.node(node).kind,
        NodeKind::Boolean { .. }
            | NodeKind::Nil { .. }
            | NodeKind::Number { .. }
            | NodeKind::String { .. }
            | NodeKind::Table { .. }
    ) {
        context.emit(
            findings,
            "constant_condition",
            tree.node(node).span,
            "literal condition has a fixed truth value",
        );
    }

    if matches!(&tree.node(node).kind, NodeKind::Unary { operator, .. }
        if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Length))
    {
        context.emit(
            findings,
            "length_as_condition",
            tree.node(node).span,
            "zero is truthy in Luau; compare the length explicitly",
        );
    }
}

fn is_nan(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(condition(tree, node)).kind, NodeKind::Binary { left, operator, right }
        if tree.token(*operator).kind == TokenKind::Symbol(Symbol::Divide)
            && super::suspicious::is_zero(tree, *left)
            && super::suspicious::is_zero(tree, *right))
}

fn is_fresh_table(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(
        tree.node(condition(tree, node)).kind,
        NodeKind::Table { .. }
    )
}

fn is_comparison(tree: &Tree<'_>, node: NodeIndex) -> bool {
    matches!(&tree.node(condition(tree, node)).kind, NodeKind::Binary { operator, .. }
    if matches!(tree.token(*operator).kind, TokenKind::Symbol(
        Symbol::Equal | Symbol::NotEqual | Symbol::LessThan | Symbol::GreaterThan
            | Symbol::LessThanOrEqual | Symbol::GreaterThanOrEqual
    )))
}

fn bad_escape(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || !matches!(bytes[0], b'\'' | b'"') {
        return false;
    }

    let mut index = 1;
    let end = bytes.len() - 1;

    while index < end {
        if bytes[index] != b'\\' {
            index += 1;
            continue;
        }

        let Some(&escaped) = bytes.get(index + 1).filter(|_| index + 1 < end) else {
            return true;
        };

        index += 2;

        match escaped {
            b'a' | b'b' | b'f' | b'n' | b'r' | b't' | b'v' | b'\\' | b'"' | b'\'' | b'\n'
            | b'\r' => {}

            b'0'..=b'9' => {
                for _ in 0..2 {
                    if bytes.get(index).is_some_and(u8::is_ascii_digit) && index < end {
                        index += 1;
                    }
                }
            }

            b'x' => {
                if index + 1 >= end || !bytes[index..=index + 1].iter().all(u8::is_ascii_hexdigit) {
                    return true;
                }

                index += 2;
            }

            b'u' => {
                if bytes.get(index) != Some(&b'{') {
                    return true;
                }

                index += 1;
                let start = index;

                while index < end && bytes[index].is_ascii_hexdigit() {
                    index += 1;
                }

                if start == index || bytes.get(index) != Some(&b'}') {
                    return true;
                }

                index += 1;
            }

            b'z' => {
                while index < end && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
            }

            _ => return true,
        }
    }

    false
}

fn is_pure_call(tree: &Tree<'_>, node: NodeIndex, ancestors: &[NodeIndex]) -> bool {
    let NodeKind::Call { callee, .. } = &tree.node(node).kind else {
        return false;
    };

    let NodeKind::Field { receiver, name, .. } = &tree.node(*callee).kind else {
        return false;
    };

    let pure = match tree.text(*receiver) {
        b"math" => matches!(
            tree.text(*name),
            b"abs" | b"floor" | b"ceil" | b"sqrt" | b"max" | b"min"
        ),

        b"string" => matches!(tree.text(*name), b"lower" | b"upper" | b"sub" | b"len"),
        _ => false,
    };

    pure && matches!(tree.node(*receiver).kind, NodeKind::Name { .. })
        && !super::suspicious::has_local(
            tree,
            tree.text(*receiver),
            tree.node(node).span.start,
            ancestors,
        )
}

fn assignment_names(tree: &Tree<'_>, node: NodeIndex) -> Option<(NodeIndex, NodeIndex)> {
    let NodeKind::Assignment {
        targets, values, ..
    } = &tree.node(node).kind
    else {
        return None;
    };

    let targets = tree.list(targets);
    let values = tree.list(values);

    if targets.len() != 1 || values.len() != 1 {
        return None;
    }

    let (target, value) = (targets[0].node, values[0].node);

    (matches!(tree.node(target).kind, NodeKind::Name { .. })
        && matches!(tree.node(value).kind, NodeKind::Name { .. }))
    .then_some((target, value))
}

fn almost_swapped(statements: &NodeList, context: &Context<'_>, findings: &mut Vec<Finding>) {
    let tree = context.tree;
    let mut previous = None;

    for statement in tree.list(statements) {
        if let Some(first) = previous
            && let (Some((left, right)), Some((other_left, other_right))) = (
                assignment_names(tree, first),
                assignment_names(tree, statement.node),
            )
            && tree.text(left) != tree.text(right)
            && tree.text(left) == tree.text(other_right)
            && tree.text(right) == tree.text(other_left)
        {
            context.emit(
                findings,
                "almost_swapped",
                tree.node(statement.node).span,
                "sequential assignments overwrite a value before swapping it",
            );
        }

        previous = Some(statement.node);
    }
}

fn call_arity(tree: &Tree<'_>, arguments: NodeIndex) -> (usize, bool) {
    if let NodeKind::Arguments { values, .. } = &tree.node(arguments).kind {
        let values = tree.list(values);

        (
            values.len(),
            values.last().is_some_and(|last| {
                matches!(
                    tree.node(last.node).kind,
                    NodeKind::Call { .. } | NodeKind::MethodCall { .. } | NodeKind::Variadic { .. }
                )
            }),
        )
    } else {
        (1, false)
    }
}

fn unused_binding(
    binding: NodeIndex,
    body: NodeIndex,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let tree = context.tree;

    let NodeKind::Binding { name, .. } = &tree.node(binding).kind else {
        return;
    };

    if context.ignored_name(tree.text(*name)) || has_read(tree, body, tree.text(*name), None) {
        return;
    }

    context.emit(
        findings,
        "unused_variable",
        tree.node(*name).span,
        "parameter or loop variable is never read",
    );
}

fn has_read(tree: &Tree<'_>, node: NodeIndex, name: &[u8], parent: Option<NodeIndex>) -> bool {
    if matches!(tree.node(node).kind, NodeKind::Name { .. }) && tree.text(node) == name {
        let declaration = parent.is_some_and(|parent| match &tree.node(parent).kind {
            NodeKind::Binding { name: binding, .. } => *binding == node,
            NodeKind::Field { name: field, .. } => *field == node,
            NodeKind::MethodCall { method, .. } => *method == node,
            NodeKind::FunctionName { .. } | NodeKind::TypeName { .. } => true,

            NodeKind::Assignment { targets, .. } => {
                tree.list(targets).iter().any(|target| target.node == node)
            }

            NodeKind::CompoundAssignment { target, .. } => *target == node,
            _ => false,
        });

        return !declaration;
    }

    tree.children(node)
        .into_iter()
        .any(|child| has_read(tree, child, name, Some(node)))
}
