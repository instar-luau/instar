use vermis::{Children, Kind, Parts, View};

use super::{Context, Finding};

pub(super) fn check(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    match node.parts() {
        Some(Parts::Block { statements }) => almost_swapped(statements, context, findings),

        Some(Parts::NumericFor {
            binding,
            step,
            body,
            ..
        }) => {
            if step.is_some_and(super::suspicious::is_zero) {
                context.emit(
                    findings,
                    "zero_step_loop",
                    node.span(),
                    "numeric loop step is zero",
                );
            }

            if context.config.unused_variable.loop_variables() {
                unused_binding(binding, body, context, findings);
            }
        }

        Some(Parts::GenericFor { bindings, body, .. }) => {
            if context.config.unused_variable.loop_variables() {
                for binding in bindings {
                    unused_binding(binding, body, context, findings);
                }
            }
        }

        Some(Parts::Function {
            parameters,
            body: Some(body),
            ..
        }) => {
            if context.config.unused_variable.parameters()
                && let Some(Parts::Parameters { parameters }) = parameters.parts()
            {
                for binding in parameters {
                    unused_binding(binding, body, context, findings);
                }
            }
        }

        Some(Parts::Binary {
            left,
            operator,
            right,
        }) if matches!(operator.text(), b"==" | b"~=") => {
            if is_nan(left) || is_nan(right) {
                context.emit(
                    findings,
                    "compare_nan",
                    node.span(),
                    "comparison with literal NaN has a fixed result",
                );
            }

            if is_fresh_table(left) || is_fresh_table(right) {
                context.emit(
                    findings,
                    "constant_table_comparison",
                    node.span(),
                    "fresh table is compared by identity",
                );
            }
        }

        Some(Parts::Call { callee, arguments }) => {
            check_call(node, callee, arguments, ancestors, context, findings);
        }

        Some(
            Parts::Branch { condition, .. }
            | Parts::While { condition, .. }
            | Parts::Repeat { condition, .. }
            | Parts::Conditional { condition, .. },
        ) => check_condition(condition, context, findings),

        Some(Parts::CallStatement { call }) if is_pure_call(call, ancestors) => {
            context.emit(
                findings,
                "must_use",
                call.span(),
                "result of a pure function is discarded",
            );
        }

        _ => {}
    }

    if node.kind() == Kind::String && bad_escape(node.text()) {
        context.emit(
            findings,
            "bad_string_escape",
            node.span(),
            "string contains an undefined escape",
        );
    }
}

fn check_call(
    node: View<'_, '_>,
    callee: View<'_, '_>,
    arguments: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if matches!(callee.text(), b"type" | b"typeof")
        && callee.kind() == Kind::Name
        && let Some(Parts::Arguments { mut values }) = arguments.parts()
        && values.next().is_some_and(is_comparison)
        && values.next().is_none()
        && !super::suspicious::has_local(callee.text(), node.span().start, ancestors)
    {
        context.emit(
            findings,
            "type_check_inside_call",
            node.span(),
            "comparison belongs outside the type check",
        );
    }

    if context.enabled("mismatched_arg_count")
        && callee.kind() == Kind::Name
        && let Some((minimum, variadic)) = context.writes.arity(callee)
    {
        let (actual, expands) = call_arity(arguments);

        if actual > minimum && !variadic || actual < minimum && !expands {
            context.emit(
                findings,
                "mismatched_arg_count",
                node.span(),
                "call argument count differs from the local function declaration",
            );
        }
    }
}

fn condition<'tree, 'source>(node: View<'tree, 'source>) -> View<'tree, 'source> {
    match node.parts() {
        Some(Parts::Group { expression }) => condition(expression),
        _ => node,
    }
}

fn check_condition(node: View<'_, '_>, context: &Context<'_>, findings: &mut Vec<Finding>) {
    let node = condition(node);

    if matches!(
        node.kind(),
        Kind::Boolean | Kind::Nil | Kind::Number | Kind::String | Kind::Table
    ) {
        context.emit(
            findings,
            "constant_condition",
            node.span(),
            "literal condition has a fixed truth value",
        );
    }

    if matches!(node.parts(), Some(Parts::Unary { operator, .. }) if operator.text() == b"#") {
        context.emit(
            findings,
            "length_as_condition",
            node.span(),
            "zero is truthy in Luau; compare the length explicitly",
        );
    }
}

fn is_nan(node: View<'_, '_>) -> bool {
    matches!(condition(node).parts(), Some(Parts::Binary { left, operator, right })
        if operator.text() == b"/"
            && super::suspicious::is_zero(left)
            && super::suspicious::is_zero(right))
}

fn is_fresh_table(node: View<'_, '_>) -> bool {
    condition(node).kind() == Kind::Table
}

fn is_comparison(node: View<'_, '_>) -> bool {
    matches!(condition(node).parts(), Some(Parts::Binary { operator, .. })
        if matches!(operator.text(), b"==" | b"~=" | b"<" | b">" | b"<=" | b">="))
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

fn is_pure_call(node: View<'_, '_>, ancestors: &[View<'_, '_>]) -> bool {
    let Some(Parts::Call { callee, .. }) = node.parts() else {
        return false;
    };

    let Some(Parts::Field { receiver, name }) = callee.parts() else {
        return false;
    };

    let pure = match receiver.text() {
        b"math" => matches!(
            name.text(),
            b"abs" | b"floor" | b"ceil" | b"sqrt" | b"max" | b"min"
        ),

        b"string" => matches!(name.text(), b"lower" | b"upper" | b"sub" | b"len"),
        _ => false,
    };

    pure && receiver.kind() == Kind::Name
        && !super::suspicious::has_local(receiver.text(), node.span().start, ancestors)
}

fn assignment_names<'tree, 'source>(
    node: View<'tree, 'source>,
) -> Option<(View<'tree, 'source>, View<'tree, 'source>)> {
    let Parts::Assignment {
        mut targets,
        operator,
        mut values,
    } = node.parts()?
    else {
        return None;
    };

    if operator.text() != b"=" {
        return None;
    }

    let (target, value) = (targets.next()?, values.next()?);

    (targets.next().is_none()
        && values.next().is_none()
        && target.kind() == Kind::Name
        && value.kind() == Kind::Name)
        .then_some((target, value))
}

fn almost_swapped(
    statements: Children<'_, '_>,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let mut previous = None;

    for statement in statements {
        if let Some(first) = previous
            && let (Some((left, right)), Some((other_left, other_right))) =
                (assignment_names(first), assignment_names(statement))
            && left.text() != right.text()
            && left.text() == other_right.text()
            && right.text() == other_left.text()
        {
            context.emit(
                findings,
                "almost_swapped",
                statement.span(),
                "sequential assignments overwrite a value before swapping it",
            );
        }

        previous = Some(statement);
    }
}

fn call_arity(arguments: View<'_, '_>) -> (usize, bool) {
    if let Some(Parts::Arguments { values }) = arguments.parts() {
        let mut count = 0;
        let mut last = None;

        for argument in values {
            count += 1;
            last = Some(argument.kind());
        }

        (
            count,
            matches!(last, Some(Kind::Call | Kind::MethodCall | Kind::Variadic)),
        )
    } else {
        (1, false)
    }
}

fn unused_binding(
    binding: View<'_, '_>,
    body: View<'_, '_>,
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let Some(Parts::Binding { name, .. }) = binding.parts() else {
        return;
    };

    if context.ignored_name(name.text()) || has_read(body, name.text(), None) {
        return;
    }

    context.emit(
        findings,
        "unused_variable",
        name.span(),
        "parameter or loop variable is never read",
    );
}

fn has_read(node: View<'_, '_>, name: &[u8], parent: Option<View<'_, '_>>) -> bool {
    if node.kind() == Kind::Name && node.text() == name {
        let declaration = parent.is_some_and(|parent| match parent.parts() {
            Some(Parts::Binding { name: binding, .. }) => binding.span() == node.span(),
            Some(Parts::Field { name: field, .. }) => field.span() == node.span(),
            Some(Parts::MethodCall { method, .. }) => method.span() == node.span(),
            Some(Parts::FunctionName { .. } | Parts::TypeName { .. }) => true,

            Some(Parts::Assignment { targets, .. }) => targets
                .into_iter()
                .any(|target| target.span() == node.span()),

            _ => false,
        });

        return !declaration;
    }

    node.children()
        .any(|child| has_read(child, name, Some(node)))
}
