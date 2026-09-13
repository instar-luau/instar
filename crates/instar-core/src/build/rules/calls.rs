use super::{Context, replace_keep_lines, span, text};

use crate::build::{
    configuration::{RemoveCalls, Rules},
    mapping::Edit,
};

use vermis::{Kind, Parts, View};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) {
    for (index, parent) in context.parents.iter().enumerate() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if settings.remove_method_call && view.kind() == Kind::MethodCall {
            method(view, edits);
        }

        if settings.convert_square_root_call && view.kind() == Kind::Call {
            square_root(view, *parent, context, edits);
        }

        if settings.remove_function_call_parens
            && matches!(view.kind(), Kind::Call | Kind::MethodCall)
        {
            parens(view, edits);
        }

        if view.kind() == Kind::CallStatement {
            if let Some(rule) = settings
                .remove_assertions
                .as_ref()
                .filter(|rule| rule.enabled())
            {
                remove_statement(view, context, rule.preserve(), &["assert"], edits);
            }

            if let Some(rule) = settings
                .remove_debug_profiling
                .as_ref()
                .filter(|rule| rule.enabled())
            {
                remove_statement(
                    view,
                    context,
                    rule.preserve(),
                    &["debug.profilebegin", "debug.profileend"],
                    edits,
                );
            }

            if let Some(rule) = &settings.remove_calls {
                remove_calls(view, context, rule, edits);
            }
        }
    }
}

fn method(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    let Some(Parts::MethodCall {
        receiver,
        method,
        arguments,
        ..
    }) = view.parts()
    else {
        return;
    };

    if receiver.kind() != Kind::Name {
        return;
    }

    edits.push(Edit {
        range: receiver.span().end..method.span().start,
        text: ".".into(),
    });

    if text(arguments).starts_with('(') {
        edits.push(Edit {
            range: arguments.span().start + 1..arguments.span().start + 1,
            text: if text(arguments) == "()" {
                text(receiver).to_owned()
            } else {
                format!("{}, ", text(receiver))
            },
        });
    } else {
        edits.push(Edit {
            range: span(arguments),
            text: format!("({}, {})", text(receiver), text(arguments)),
        });
    }
}

fn square_root(
    view: View<'_, '_>,
    parent: Option<usize>,
    context: &Context<'_, '_, '_, '_, '_>,
    edits: &mut Vec<Edit>,
) {
    if parent.is_some_and(|parent| context.tree.nodes[parent].kind == Kind::CallStatement) {
        return;
    }

    let Some(Parts::Call { callee, arguments }) = view.parts() else {
        return;
    };

    let Some(Parts::Field { receiver, name }) = callee.parts() else {
        return;
    };

    let Some(Parts::Arguments { mut values }) = arguments.parts() else {
        return;
    };

    if values.next().is_none()
        || values.next().is_some()
        || text(receiver) != "math"
        || text(name) != "sqrt"
    {
        return;
    }

    edits.push(Edit {
        range: span(callee),
        text: "(".into(),
    });

    edits.push(Edit {
        range: view.span().end..view.span().end,
        text: " ^ 0.5)".into(),
    });
}

fn parens(view: View<'_, '_>, edits: &mut Vec<Edit>) {
    let Some(Parts::Call { arguments, .. } | Parts::MethodCall { arguments, .. }) = view.parts()
    else {
        return;
    };

    let Some(Parts::Arguments { mut values }) = arguments.parts() else {
        return;
    };

    let Some(value) = values.next() else {
        return;
    };

    if values.next().is_some()
        || !matches!(value.kind(), Kind::String | Kind::Table)
        || !text(arguments).starts_with('(')
        || !text(arguments).ends_with(')')
    {
        return;
    }

    edits.push(Edit {
        range: arguments.span().start..arguments.span().start + 1,
        text: String::new(),
    });

    edits.push(Edit {
        range: arguments.span().end - 1..arguments.span().end,
        text: String::new(),
    });
}

fn call<'tree, 'source>(
    view: View<'tree, 'source>,
) -> Option<(View<'tree, 'source>, View<'tree, 'source>)> {
    let Parts::CallStatement { call } = view.parts()? else {
        return None;
    };

    let Parts::Call { callee, arguments } = call.parts()? else {
        return None;
    };

    Some((callee, arguments))
}

fn path(view: View<'_, '_>) -> Option<String> {
    match view.kind() {
        Kind::Name => Some(text(view).to_owned()),

        Kind::Field => {
            let Parts::Field { receiver, name } = view.parts()? else {
                return None;
            };

            Some(format!("{}.{}", path(receiver)?, text(name)))
        }

        _ => None,
    }
}

fn has_call(view: View<'_, '_>) -> bool {
    if matches!(view.kind(), Kind::Call | Kind::MethodCall) {
        return true;
    }

    view.kind() != Kind::Function && view.children().any(has_call)
}

fn remove_statement(
    view: View<'_, '_>,
    context: &Context<'_, '_, '_, '_, '_>,
    preserve: bool,
    names: &[&str],
    edits: &mut Vec<Edit>,
) {
    let Some((callee, arguments)) = call(view) else {
        return;
    };

    let Some(name) = path(callee) else {
        return;
    };

    if !names.contains(&name.as_str()) || preserve && has_call(arguments) {
        return;
    }

    replace_keep_lines(context.text, span(view), "", edits);
}

fn remove_calls(
    view: View<'_, '_>,
    context: &Context<'_, '_, '_, '_, '_>,
    rule: &RemoveCalls,
    edits: &mut Vec<Edit>,
) {
    let names = rule
        .functions()
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    remove_statement(view, context, rule.preserve(), &names, edits);
}
