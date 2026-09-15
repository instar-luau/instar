use super::super::syntax::{
    Context, array, comparison, expands, field, global, kind, local, number, unwrap,
};

use serde_json::Value;
use std::collections::BTreeMap;

const PURE: &[&str] = &[
    "math.abs",
    "math.ceil",
    "math.clamp",
    "math.floor",
    "math.max",
    "math.min",
    "math.round",
    "math.sqrt",
    "os.date",
    "os.time",
    "rawequal",
    "rawget",
    "rawlen",
    "select",
    "string.byte",
    "string.char",
    "string.find",
    "string.format",
    "string.gsub",
    "string.len",
    "string.lower",
    "string.match",
    "string.rep",
    "string.reverse",
    "string.split",
    "string.sub",
    "string.upper",
    "table.concat",
    "table.find",
    "table.pack",
    "tonumber",
    "tostring",
    "type",
    "typeof",
    "utf8.char",
    "utf8.len",
];

pub(super) fn invocation(
    context: &mut Context<'_>,
    value: &Value,
    parent: &Value,
    functions: &BTreeMap<&str, &Value>,
) {
    let function = &value["func"];
    let arguments = array(&value["args"]);
    let path = global(function).unwrap_or_default();

    if kind(parent) == "AstStatExpr" {
        if PURE.contains(&path.as_str()) {
            context.emit(
                "discarded_return",
                value,
                format!("The result of {path} is discarded"),
            );
        }

        if matches!(path.as_str(), "pcall" | "xpcall") {
            context.emit(
                "ignored_protected_call",
                value,
                "The protected call discards its status and results",
            );
        }
    }

    if path == "type" && arguments.first().is_some_and(comparison) {
        context.emit(
            "misplaced_type_comparison",
            value,
            "Move the comparison outside type()",
        );
    }

    if path == "require"
        && let Some(argument) = arguments.first()
        && kind(argument) == "AstExprConstantString"
        && let Some(reason) = context
            .settings
            .options
            .restricted_import
            .paths
            .get(field(argument, "value"))
    {
        context.emit(
            "restricted_import",
            argument,
            format!("This import is restricted: {reason}"),
        );
    }

    deprecated(context, value, &path);

    if let Some(identity) = local(function)
        && !context.writes.contains(identity)
        && let Some(declaration) = functions.get(identity)
        && declaration["vararg"] != true
        && !arguments.last().is_some_and(expands)
    {
        let parameters = array(&declaration["args"]);

        let required = !parameters.is_empty()
            && parameters.iter().all(|parameter| {
                !parameter["luauType"].is_null()
                    && !context
                        .text(&parameter["luauType"])
                        .trim_end()
                        .ends_with('?')
            });

        if arguments.len() > parameters.len() || (required && arguments.len() < parameters.len()) {
            context.emit(
                "argument_count",
                value,
                format!(
                    "Function declares {} parameters but receives {} arguments",
                    parameters.len(),
                    arguments.len()
                ),
            );
        }
    }

    if matches!(path.as_str(), "table.insert" | "table.remove")
        && !arguments.last().is_some_and(expands)
    {
        let valid = if path == "table.insert" {
            (2..=3).contains(&arguments.len())
        } else {
            (1..=2).contains(&arguments.len())
        };

        let indexed = arguments.len() == if path == "table.insert" { 3 } else { 2 };

        if !valid
            || indexed
                && number(&arguments[1]).is_some_and(|index| index < 1.0 || index.fract() != 0.0)
        {
            context.emit(
                "invalid_table_operation",
                value,
                "Invalid table operation arguments or index",
            );
        }
    }

    format_string(context, value, &path);
    super::roblox::apply(context, value, &path);
}

fn deprecated(context: &mut Context<'_>, value: &Value, path: &str) {
    let replacement = [
        ("delay", "task.delay"),
        ("elapsedTime", "os.clock"),
        ("spawn", "task.spawn"),
        ("table.foreach", "a for loop"),
        ("table.foreachi", "a for loop"),
        ("table.getn", "the length operator"),
        ("wait", "task.wait"),
    ]
    .into_iter()
    .find(|(name, _)| *name == path)
    .map(|(_, replacement)| replacement.to_owned())
    .or_else(|| {
        context
            .settings
            .options
            .deprecated_function
            .additional
            .get(path)
            .cloned()
    });

    if let Some(replacement) = replacement {
        context.emit(
            "deprecated_function",
            value,
            format!("{path} is deprecated; use {replacement}"),
        );
    }

    if context.roblox && value["self"] == true {
        let name = field(&value["func"], "index");

        if let Some((_, replacement)) = [
            ("Remove", "Destroy"),
            ("children", "GetChildren"),
            ("findFirstChild", "FindFirstChild"),
            ("getChildren", "GetChildren"),
        ]
        .into_iter()
        .find(|(original, _)| *original == name)
        {
            context.emit(
                "deprecated_function",
                value,
                format!("{name} is deprecated; use {replacement}"),
            );
        }
    }
}

fn format_string(context: &mut Context<'_>, value: &Value, path: &str) {
    let arguments = array(&value["args"]);

    let literal = if matches!(path, "string.format" | "os.date") {
        arguments.first()
    } else if value["self"] == true && field(&value["func"], "index") == "format" {
        Some(unwrap(&value["func"]["expr"]))
    } else {
        None
    };

    let Some(literal) = literal.filter(|literal| kind(literal) == "AstExprConstantString") else {
        return;
    };

    let text = field(literal, "value");
    let mut characters = text.chars();

    while let Some(character) = characters.next() {
        if character != '%' {
            continue;
        }

        let Some(mut conversion) = characters.next() else {
            context.emit(
                "invalid_format_string",
                literal,
                "Unfinished format conversion",
            );

            break;
        };

        if conversion == '%' {
            continue;
        }

        if path != "os.date" {
            while "-+ #0.123456789".contains(conversion) {
                let Some(next) = characters.next() else {
                    context.emit(
                        "invalid_format_string",
                        literal,
                        "Unfinished format conversion",
                    );

                    return;
                };

                conversion = next;
            }
        }

        let allowed = if path == "os.date" {
            "aAbBcdHIjmMpSUwWxXyYZzCeFgGhRrTtDuVn%"
        } else {
            "cdiouxXeEfgGqs"
        };

        if !allowed.contains(conversion) {
            context.emit(
                "invalid_format_string",
                literal,
                format!("Invalid format conversion %{conversion}"),
            );
        }
    }
}
