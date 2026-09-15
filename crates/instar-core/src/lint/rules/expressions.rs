use super::super::syntax::{Context, array, call, comparison, field, global, kind, number, unwrap};

use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn table(context: &mut Context<'_>, value: &Value) {
    let mut keys = BTreeSet::new();
    let mut positional = false;
    let mut named = false;

    for item in array(&value["items"]) {
        if field(item, "kind") == "item" {
            positional = true;
            continue;
        }

        named = true;
        let key = &item["key"];

        let identity = match kind(key) {
            "AstExprConstantString" => Some(format!("string:{}", field(key, "value"))),
            "AstExprConstantNumber" => number(key).map(|number| format!("number:{number}")),
            "AstExprConstantBool" => Some(format!("boolean:{}", key["value"])),
            _ => None,
        };

        if let Some(identity) = identity
            && !keys.insert(identity)
        {
            context.emit(
                "duplicate_table_key",
                key,
                "This key overwrites an earlier table entry",
            );
        }
    }

    if positional && named {
        context.emit(
            "mixed_table",
            value,
            "This table mixes positional and named entries",
        );
    }
}

pub(super) fn binary(context: &mut Context<'_>, value: &Value) {
    let left = &value["left"];
    let right = &value["right"];
    let operator = field(value, "op");

    if matches!(operator, "Div" | "FloorDiv" | "Mod") && number(right) == Some(0.0) {
        context.emit("division_by_zero", value, "The divisor is zero");
    }

    if comparison(value) {
        if [left, right].iter().any(|operand| {
            global(operand).as_deref() == Some("math.nan")
                || (field(operand, "op") == "Div"
                    && number(&operand["left"]) == Some(0.0)
                    && number(&operand["right"]) == Some(0.0))
        }) {
            context.emit(
                "not_a_number_comparison",
                value,
                "A not-a-number value is unequal to every value, including itself",
            );
        }

        if [left, right]
            .iter()
            .any(|operand| kind(unwrap(operand)) == "AstExprTable")
        {
            context.emit(
                "table_identity_comparison",
                value,
                "A table literal has a fresh identity",
            );
        }

        if comparison(left)
            || comparison(right)
            || (kind(left) == "AstExprUnary" && field(left, "op") == "Not")
        {
            context.emit(
                "comparison_precedence",
                value,
                "Parenthesize the intended comparison",
            );
        }

        type_name(context, left, right);
        type_name(context, right, left);
    }

    if operator == "Or" && field(left, "op") == "And" {
        context.emit(
            "logical_conditional",
            value,
            "Use a conditional statement for value selection",
        );

        if super::super::syntax::truth(&left["right"]) == Some(false) || comparison(&left["right"])
        {
            context.emit(
                "misleading_conditional",
                value,
                "A false middle result selects the fallback instead",
            );
        }
    }
}

fn type_name(context: &mut Context<'_>, query: &Value, literal: &Value) {
    if kind(literal) != "AstExprConstantString" || !(call(query, "type") || call(query, "typeof")) {
        return;
    }

    let name = field(literal, "value");

    if context.roblox && call(query, "typeof") && name.chars().any(char::is_uppercase) {
        return;
    }

    if ![
        "nil", "boolean", "number", "string", "table", "function", "thread", "userdata", "vector",
        "buffer",
    ]
    .contains(&name)
    {
        context.emit(
            "invalid_type_name",
            literal,
            format!("{name} is not a type-query result"),
        );
    }
}

pub(super) fn escapes(context: &mut Context<'_>, value: &Value) {
    let text = context.text(value).to_owned();

    if !text.starts_with(['\'', '"']) {
        return;
    }

    let mut characters = text.char_indices();

    while let Some((index, character)) = characters.next() {
        if character != '\\' {
            continue;
        }

        let Some((_, escaped)) = characters.next() else {
            break;
        };

        if !matches!(
            escaped,
            'a' | 'b' | 'f' | 'n' | 'r' | 't' | 'v' | '\\' | '\'' | '"' | 'z' | 'x' | 'u' | '0'
                ..='9' | '\r' | '\n'
        ) && let Some(range) = context.span(value)
        {
            context.emit_range(
                "invalid_string_escape",
                range.start + index..range.start + index + 1 + escaped.len_utf8(),
                format!("Undefined escape \\{escaped}"),
                Vec::new(),
            );
        }
    }
}

pub(super) fn literal(context: &mut Context<'_>, value: &Value) {
    let text = context.text(value).replace('_', "");

    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .map(|digits| (digits, 16))
        .or_else(|| {
            text.strip_prefix("0b")
                .or_else(|| text.strip_prefix("0B"))
                .map(|digits| (digits, 2))
        });

    if digits.is_some_and(|(digits, radix)| {
        !digits.contains('.') && u64::from_str_radix(digits, radix).is_err()
    }) {
        context.emit(
            "number_literal_overflow",
            value,
            "This integer literal exceeds 64 bits",
        );
    }
}
