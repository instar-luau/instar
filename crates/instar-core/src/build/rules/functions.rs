use super::{Context, text};
use crate::build::{configuration::Rules, mapping::Edit};
use vermis::{Kind, Parts, View};

pub(super) fn apply(
    context: &Context<'_, '_, '_, '_, '_>,
    settings: &Rules,
    edits: &mut Vec<Edit>,
) {
    for index in 0..context.tree.nodes.len() {
        let Some(view) = context.tree.view(index) else {
            continue;
        };

        if settings.convert_function_to_assignment && view.kind() == Kind::Function {
            assignment(view, context, false, edits);
        } else if settings.remove_method_definition && view.kind() == Kind::Function {
            method(view, context, edits);
        }

        if settings.convert_local_function_to_assign && view.kind() == Kind::LocalFunction {
            assignment(view, context, true, edits);
        }
    }
}

fn assignment(
    view: View<'_, '_>,
    context: &Context<'_, '_, '_, '_, '_>,
    local: bool,
    edits: &mut Vec<Edit>,
) {
    let Some(Parts::Function {
        attributes,
        name: Some(name),
        parameters,
        body,
        ..
    }) = view.parts()
    else {
        return;
    };

    if attributes.is_some() || local && body.is_some_and(|body| references_name(body, text(name))) {
        return;
    }

    let method = matches!(
        name.parts(),
        Some(Parts::FunctionName {
            method: Some(_),
            ..
        })
    );

    let name_text = text(name).replace(':', ".");

    let Some(function) = context.tree.tokens.iter().find(|token| {
        token.span.start >= view.span().start
            && token.span.end <= name.span().start
            && token.utf8(context.tree.source).ok() == Some("function")
    }) else {
        return;
    };

    let start = if local {
        context
            .tree
            .tokens
            .iter()
            .find(|token| {
                token.span.start >= view.span().start
                    && token.span.end <= function.span.start
                    && token.utf8(context.tree.source).ok() == Some("local")
            })
            .map_or(function.span.start, |token| token.span.start)
    } else {
        function.span.start
    };

    edits.push(Edit {
        range: start..name.span().end,
        text: format!(
            "{}{} = function",
            if local { "local " } else { "" },
            name_text
        ),
    });

    if method {
        insert_self(parameters, edits);
    }
}

fn references_name(view: View<'_, '_>, name: &str) -> bool {
    view.kind() == Kind::Name && text(view) == name
        || view.children().any(|child| references_name(child, name))
}

fn method(view: View<'_, '_>, context: &Context<'_, '_, '_, '_, '_>, edits: &mut Vec<Edit>) {
    let Some(Parts::Function {
        name: Some(name),
        parameters,
        ..
    }) = view.parts()
    else {
        return;
    };

    let Some(Parts::FunctionName {
        method: Some(method),
        ..
    }) = name.parts()
    else {
        return;
    };

    let Some(colon) = context.tree.tokens.iter().find(|token| {
        token.span.start >= name.span().start
            && token.span.end <= method.span().start
            && token.utf8(context.tree.source).ok() == Some(":")
    }) else {
        return;
    };

    edits.push(Edit {
        range: colon.span.start..colon.span.end,
        text: ".".into(),
    });

    insert_self(parameters, edits);
}

fn insert_self(parameters: View<'_, '_>, edits: &mut Vec<Edit>) {
    let Some(Parts::Parameters {
        parameters: mut values,
    }) = parameters.parts()
    else {
        return;
    };

    edits.push(Edit {
        range: parameters.span().start + 1..parameters.span().start + 1,
        text: if values.next().is_some() {
            "self, ".into()
        } else {
            "self".into()
        },
    });
}
