use super::super::syntax::{Context, array, call, field, global, kind};

use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn declaration(context: &mut Context<'_>, value: &Value) {
    let bindings = array(&value["vars"]);
    let values = array(&value["values"]);
    duplicates(context, bindings);

    if !values.is_empty() {
        balance(context, value, bindings, values);
    }

    for binding in bindings {
        if values.is_empty() {
            if binding["luauType"].is_null() {
                context.emit(
                    "untyped_local",
                    binding,
                    "This local has neither a value nor a type annotation",
                );
            }

            if !context.writes.contains(field(binding, "location")) {
                context.emit(
                    "uninitialized_local",
                    binding,
                    "This local is never initialized",
                );
            }
        }
    }

    if values.is_empty()
        || bindings.iter().any(|binding| {
            binding["isConst"] == true || context.writes.contains(field(binding, "location"))
        })
    {
        return;
    }

    if context
        .settings
        .options
        .constant_binding
        .mutated_tables_stay_local
        && bindings
            .iter()
            .any(|binding| context.mutated.contains(field(binding, "location")))
    {
        return;
    }

    let imported = bindings.len() == 1
        && values.len() == 1
        && bindings[0]["luauType"].is_null()
        && call(&values[0], "require");

    let rule = if imported
        && context.settings.level("constant_import") != super::super::configuration::Level::Allow
    {
        "constant_import"
    } else {
        "constant_binding"
    };

    if let Some(range) = context.span(value)
        && context.text(value).starts_with("local")
    {
        context.emit_range(
            rule,
            range.clone(),
            "This binding can use const",
            vec![super::super::Edit {
                start: range.start,
                end: range.start + "local".len(),
                text: "const".into(),
            }],
        );
    }
}

pub(super) fn duplicates(context: &mut Context<'_>, bindings: &[Value]) {
    let mut names = BTreeSet::new();

    for binding in bindings {
        let name = field(binding, "name");

        if name != "_" && !names.insert(name) {
            context.emit(
                "duplicate_binding",
                binding,
                format!("{name} is declared twice"),
            );
        }
    }
}

fn balance(context: &mut Context<'_>, node: &Value, targets: &[Value], values: &[Value]) {
    if targets.len() != values.len() && !values.last().is_some_and(super::super::syntax::expands) {
        context.emit(
            "unbalanced_assignment",
            node,
            format!("{} targets receive {} values", targets.len(), values.len()),
        );
    }
}

pub(super) fn assignment(context: &mut Context<'_>, value: &Value) {
    let targets = array(&value["vars"]);
    let values = array(&value["values"]);
    balance(context, value, targets, values);

    for (target, source) in targets.iter().zip(values) {
        if context.same(target, source) {
            context.emit(
                "self_assignment",
                target,
                "This assignment writes a value back to itself",
            );
        }

        if kind(target) == "AstExprGlobal" {
            let name = field(target, "global");

            context.emit(
                if context.globals.contains(name) {
                    "builtin_assignment"
                } else {
                    "global_assignment"
                },
                target,
                format!("Assignment writes global {name}"),
            );
        }
    }
}

pub(super) fn globals(context: &mut Context<'_>, value: &Value, parent: &Value) {
    let name = field(value, "global");

    if name == "_G" {
        context.emit(
            "global_environment",
            value,
            "Access to the global environment",
        );
    }

    if let Some(reason) = context.settings.options.restricted_global.get(name) {
        context.emit(
            "restricted_global",
            value,
            format!("{name} is restricted: {reason}"),
        );
    }

    let assigned = kind(parent) == "AstStatAssign"
        && array(&parent["vars"])
            .iter()
            .any(|target| std::ptr::eq(target, value));

    if assigned || kind(parent) == "AstStatFunction" {
        return;
    }

    let declared = context.nodes.iter().any(|node| {
        (kind(node.value) == "AstStatFunction"
            && global(&node.value["name"]).as_deref() == Some(name))
            || (kind(node.value) == "AstStatAssign"
                && array(&node.value["vars"])
                    .iter()
                    .any(|target| global(target).as_deref() == Some(name)))
    });

    if !declared && !context.globals.contains(name) {
        context.emit(
            "undefined_variable",
            value,
            format!("Unknown global '{name}'"),
        );
    }
}
