use vermis::{Kind, Parts, View};

use super::{Context, Finding};

fn literal_argument(arguments: View<'_, '_>) -> bool {
    matches!(arguments.parts(), Some(Parts::Arguments { values })
        if values.clone().count() == 1
            && values.clone().next().is_some_and(|value| value.kind() == Kind::String))
}

fn repeated_in_loop(node: View<'_, '_>, ancestors: &[View<'_, '_>]) -> bool {
    for ancestor in ancestors.iter().rev() {
        if matches!(ancestor.kind(), Kind::Function | Kind::LocalFunction) {
            return false;
        }

        let Some(
            Parts::While { body, .. }
            | Parts::Repeat { body, .. }
            | Parts::NumericFor { body, .. }
            | Parts::GenericFor { body, .. },
        ) = ancestor.parts()
        else {
            continue;
        };

        if body.span().start <= node.span().start && node.span().end <= body.span().end {
            return true;
        }
    }

    false
}

fn manual_clone(node: View<'_, '_>) -> bool {
    let Some(Parts::GenericFor {
        mut bindings,
        mut values,
        body,
    }) = node.parts()
    else {
        return false;
    };

    let (Some(key), Some(value), None) = (bindings.next(), bindings.next(), bindings.next()) else {
        return false;
    };

    let (Some(Parts::Binding { name: key, .. }), Some(Parts::Binding { name: value, .. })) =
        (key.parts(), value.parts())
    else {
        return false;
    };

    let Some(iterator) = values.next() else {
        return false;
    };

    if values.next().is_some() {
        return false;
    }

    let Some(Parts::Call { callee, arguments }) = iterator.parts() else {
        return false;
    };

    if callee.kind() != Kind::Name || callee.text() != b"pairs" {
        return false;
    }

    let Some(Parts::Arguments { mut values }) = arguments.parts() else {
        return false;
    };

    let Some(source) = values.next() else {
        return false;
    };

    if values.next().is_some() || source.kind() != Kind::Name {
        return false;
    }

    let Some(Parts::Block { mut statements }) = body.parts() else {
        return false;
    };

    let Some(statement) = statements.next() else {
        return false;
    };

    if statements.next().is_some() {
        return false;
    }

    let Some(Parts::Assignment {
        mut targets,
        operator,
        mut values,
    }) = statement.parts()
    else {
        return false;
    };

    let (Some(target), Some(item)) = (targets.next(), values.next()) else {
        return false;
    };

    if targets.next().is_some() || values.next().is_some() || operator.text() != b"=" {
        return false;
    }

    let Some(Parts::Index {
        receiver,
        key: assigned_key,
    }) = target.parts()
    else {
        return false;
    };

    receiver.kind() == Kind::Name
        && receiver.text() != source.text()
        && assigned_key.text() == key.text()
        && item.kind() == Kind::Name
        && item.text() == value.text()
}

fn accumulated_concat(node: View<'_, '_>, ancestors: &[View<'_, '_>]) -> bool {
    let Some(Parts::Binary { left, operator, .. }) = node.parts() else {
        return false;
    };

    if operator.text() != b".." || left.kind() != Kind::Name {
        return false;
    }

    ancestors.iter().rev().any(|ancestor| {
        let Some(Parts::Assignment {
            targets, values, ..
        }) = ancestor.parts()
        else {
            return false;
        };

        let (Some(target), Some(value)) = (targets.into_iter().next(), values.into_iter().next())
        else {
            return false;
        };

        value.span().start <= node.span().start
            && value.span().end >= node.span().end
            && target.kind() == Kind::Name
            && target.text() == left.text()
    })
}

pub(super) fn check(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    let in_loop = repeated_in_loop(node, ancestors);

    if in_loop {
        match node.parts() {
            Some(Parts::Call { callee, arguments })
                if context.enabled("loop_invariant_call")
                    && callee.kind() == Kind::Name
                    && callee.text() == b"require"
                    && !super::suspicious::has_local(b"require", node.span().start, ancestors)
                    && literal_argument(arguments) =>
            {
                context.emit(
                    findings,
                    "loop_invariant_call",
                    node.span(),
                    "literal require call repeated inside a loop",
                );
            }

            Some(Parts::MethodCall {
                receiver,
                method,
                arguments,
                ..
            }) if context.enabled("loop_invariant_call")
                && receiver.kind() == Kind::Name
                && receiver.text() == b"game"
                && method.text() == b"GetService"
                && !super::suspicious::has_local(b"game", node.span().start, ancestors)
                && literal_argument(arguments) =>
            {
                context.emit(
                    findings,
                    "loop_invariant_call",
                    node.span(),
                    "GetService call repeated inside a loop",
                );
            }

            _ => {}
        }

        if context.enabled("string_concat_in_loop") && accumulated_concat(node, ancestors) {
            context.emit(
                findings,
                "string_concat_in_loop",
                node.span(),
                "repeated concatenation grows a string quadratically",
            );
        }
    }

    if node.kind() == Kind::GenericFor
        && context.enabled("manual_table_clone")
        && !super::suspicious::has_local(b"pairs", node.span().start, ancestors)
        && manual_clone(node)
    {
        context.emit(
            findings,
            "manual_table_clone",
            node.span(),
            "table-copy loop may be replaced with table.clone",
        );
    }
}
