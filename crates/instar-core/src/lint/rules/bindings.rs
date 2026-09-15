use super::super::{
    Edit,
    syntax::{Context, array, call, field, kind, local},
};

use serde_json::Value;

pub(super) fn binding(context: &mut Context<'_>, value: &Value, parent: &Value, scope: &Value) {
    let name = field(value, "name");
    let identity = field(value, "location");
    let parameter = kind(parent) == "AstExprFunction";

    if context.globals.contains(name) {
        context.emit(
            "shadowed_builtin",
            value,
            format!("{name} hides a standard global"),
        );
    }

    let shadowed = context.nodes.iter().any(|node| {
        kind(node.value) == "AstLocal"
            && field(node.value, "name") == name
            && field(node.value, "location") != identity
            && context
                .span(node.value)
                .zip(context.span(value))
                .is_some_and(|(previous, current)| previous.start < current.start)
            && context
                .span(node.scope)
                .zip(context.span(value))
                .is_some_and(|(outer, current)| {
                    outer.start <= current.start && current.end <= outer.end
                })
    });

    if shadowed {
        context.emit(
            "shadowed_binding",
            value,
            format!("{name} hides a binding still in scope"),
        );
    }

    if parameter && value["luauType"].is_null() {
        context.emit(
            "untyped_parameter",
            value,
            format!("{name} has no annotation"),
        );
    }

    unused(context, value, parent, scope);
}

fn unused(context: &mut Context<'_>, value: &Value, parent: &Value, scope: &Value) {
    let name = field(value, "name");
    let identity = field(value, "location");
    let parameter = kind(parent) == "AstExprFunction";
    let loop_variable = matches!(kind(parent), "AstStatFor" | "AstStatForIn");
    let options = &context.settings.options.unused_variable;
    let ignored = crate::luau::matches(&options.ignore_pattern, name).unwrap_or(false);

    if ignored || (parameter && !options.parameters) || (loop_variable && !options.loop_variables) {
        return;
    }

    let unused = context.reads.get(identity).is_none_or(|reads| {
        reads.is_empty()
            || (kind(parent) == "AstStatLocalFunction"
                && reads.iter().all(|read| {
                    context
                        .span(read)
                        .zip(context.span(&parent["func"]))
                        .is_some_and(|(read, function)| {
                            function.start <= read.start && read.end <= function.end
                        })
                }))
    });

    if !unused {
        return;
    }

    let imported = kind(parent) == "AstStatLocal"
        && array(&parent["vars"])
            .iter()
            .position(|binding| field(binding, "location") == identity)
            .and_then(|index| array(&parent["values"]).get(index))
            .is_some_and(|value| call(value, "require"));

    let rule = if kind(parent) == "AstStatLocalFunction" {
        "unused_function"
    } else if imported {
        "unused_import"
    } else {
        "unused_variable"
    };

    let replacement = format!("_{name}");

    let collision = context.globals.contains(&replacement)
        || context.nodes.iter().any(|node| {
            (kind(node.value) == "AstExprGlobal" && field(node.value, "global") == replacement)
                || (kind(node.value) == "AstLocal"
                    && field(node.value, "name") == replacement
                    && context
                        .span(node.scope)
                        .zip(context.span(scope))
                        .is_some_and(|(left, right)| {
                            left.start < right.end && right.start < left.end
                        }))
        });

    let mut edits = Vec::new();

    if !collision
        && crate::luau::matches(
            &context.settings.options.unused_variable.ignore_pattern,
            &replacement,
        )
        .unwrap_or(false)
    {
        if let Some(range) = context.span(value) {
            edits.push(Edit {
                start: range.start,
                end: range.start + name.len(),
                text: replacement.clone(),
            });
        }

        for node in &context.nodes {
            if local(node.value) == Some(identity)
                && let Some(range) = context.span(node.value)
            {
                edits.push(Edit {
                    start: range.start,
                    end: range.end,
                    text: replacement.clone(),
                });
            }
        }
    }

    if let Some(range) = context.span(value) {
        context.emit_range(rule, range, format!("{name} is unused"), edits);
    }
}
