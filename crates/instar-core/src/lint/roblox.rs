use vermis::{Kind, Parts, View};

use super::{Context, Finding};

fn numeric_literal(node: View<'_, '_>) -> Option<f64> {
    if node.kind() == Kind::Number {
        return std::str::from_utf8(node.text()).ok()?.parse().ok();
    }

    if let Some(Parts::Unary { operator, operand }) = node.parts()
        && operator.text() == b"-"
    {
        return numeric_literal(operand).map(|number| -number);
    }

    None
}

fn is_color3_new(node: View<'_, '_>) -> bool {
    let Some(Parts::Call { callee, .. }) = node.parts() else {
        return false;
    };

    let Some(Parts::Field { receiver, name }) = callee.parts() else {
        return false;
    };

    receiver.kind() == Kind::Name && receiver.text() == b"Color3" && name.text() == b"new"
}

fn is_udim2_new<'tree, 'source>(node: View<'tree, 'source>) -> Option<View<'tree, 'source>> {
    let Some(Parts::Call { callee, arguments }) = node.parts() else {
        return None;
    };

    let Some(Parts::Field { receiver, name }) = callee.parts() else {
        return None;
    };

    (receiver.kind() == Kind::Name && receiver.text() == b"UDim2" && name.text() == b"new")
        .then_some(arguments)
}

pub(super) fn check(
    node: View<'_, '_>,
    ancestors: &[View<'_, '_>],
    context: &Context<'_>,
    findings: &mut Vec<Finding>,
) {
    if is_color3_new(node)
        && context.enabled("roblox_incorrect_color3_new_bounds")
        && !super::suspicious::has_local(b"Color3", node.span().start, ancestors)
        && let Some(Parts::Call { arguments, .. }) = node.parts()
        && let Some(Parts::Arguments { mut values }) = arguments.parts()
        && values.any(|value| {
            numeric_literal(value).is_some_and(|channel| !(0.0..=1.0).contains(&channel))
        })
    {
        context.emit(
            findings,
            "roblox_incorrect_color3_new_bounds",
            node.span(),
            "Color3.new channels use a 0–1 scale",
        );
    }

    if !super::suspicious::has_local(b"UDim2", node.span().start, ancestors)
        && let Some(arguments) = is_udim2_new(node)
    {
        let Some(Parts::Arguments { values }) = arguments.parts() else {
            return;
        };

        let count = values.clone().count();

        if count == 2 && context.enabled("roblox_suspicious_udim2_new") {
            context.emit(
                findings,
                "roblox_suspicious_udim2_new",
                node.span(),
                "UDim2.new expects scale and offset components for both axes",
            );
        } else if count == 4 && context.enabled("roblox_manual_fromscale_or_fromoffset") {
            let mut values = values;

            if let (Some(x_scale), Some(x_offset), Some(y_scale), Some(y_offset)) =
                (values.next(), values.next(), values.next(), values.next())
            {
                let scale = numeric_literal(x_offset).is_some_and(|value| value == 0.0)
                    && numeric_literal(y_offset).is_some_and(|value| value == 0.0);

                let offset = numeric_literal(x_scale).is_some_and(|value| value == 0.0)
                    && numeric_literal(y_scale).is_some_and(|value| value == 0.0);

                if scale || offset {
                    context.emit(
                        findings,
                        "roblox_manual_fromscale_or_fromoffset",
                        node.span(),
                        if scale {
                            "use UDim2.fromScale when both offsets are zero"
                        } else {
                            "use UDim2.fromOffset when both scales are zero"
                        },
                    );
                }
            }
        }
    }

    if context.enabled("roblox_prefer_get_players")
        && let Some(Parts::MethodCall {
            receiver, method, ..
        }) = node.parts()
        && method.text() == b"GetChildren"
        && !super::suspicious::has_local(b"game", node.span().start, ancestors)
        && (is_confirmed_players(receiver)
            || (receiver.kind() == Kind::Name && players_binding(receiver, ancestors)))
    {
        context.emit(
            findings,
            "roblox_prefer_get_players",
            node.span(),
            "use Players:GetPlayers() to select players",
        );
    }
}

fn is_confirmed_players(node: View<'_, '_>) -> bool {
    let Some(Parts::MethodCall {
        receiver,
        method,
        arguments,
        ..
    }) = node.parts()
    else {
        return false;
    };

    if method.text() != b"GetService" || receiver.kind() != Kind::Name || receiver.text() != b"game"
    {
        return false;
    }

    matches!(arguments.parts(), Some(Parts::Arguments { values })
    if values.clone().count() == 1
        && values.clone().next().is_some_and(|name| {
            name.kind() == Kind::String && matches!(name.text(), b"\"Players\"" | b"'Players'")
        }))
}

fn players_binding(node: View<'_, '_>, ancestors: &[View<'_, '_>]) -> bool {
    for ancestor in ancestors.iter().rev() {
        let Some(Parts::Block { statements }) = ancestor.parts() else {
            continue;
        };

        let mut declared = None;

        for statement in statements.filter(|statement| statement.span().end <= node.span().start) {
            match statement.parts() {
                Some(Parts::Local { bindings, values }) => {
                    for (binding, value) in bindings.zip(values) {
                        if let Some(Parts::Binding { name, .. }) = binding.parts()
                            && name.text() == node.text()
                        {
                            declared = Some(is_confirmed_players(value));
                        }
                    }
                }

                Some(Parts::Assignment { targets, .. }) if declared.is_some() => {
                    if targets
                        .into_iter()
                        .any(|target| target.kind() == Kind::Name && target.text() == node.text())
                    {
                        declared = Some(false);
                    }
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
