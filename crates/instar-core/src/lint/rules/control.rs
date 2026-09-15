use super::super::syntax::{
    Context, array, call, field, global, kind, local, number, truth, unwrap,
};

use serde_json::Value;
use std::collections::BTreeSet;

pub(super) fn condition(context: &mut Context<'_>, value: &Value) {
    if kind(value) == "AstExprGroup" {
        context.fix(
            "parenthesized_condition",
            value,
            context.text(&value["expr"]).into(),
        );
    }

    if kind(unwrap(value)) == "AstExprUnary" && field(unwrap(value), "op") == "Len" {
        context.emit(
            "length_condition",
            value,
            "Lengths are numbers; zero is truthy",
        );
    }
}

pub(super) fn conditional(context: &mut Context<'_>, value: &Value) {
    let test = &value["condition"];
    let body = &value["thenbody"];
    let alternative = &value["elsebody"];
    condition(context, test);

    if truth(test).is_some() {
        context.emit("constant_condition", test, "This condition is constant");
    }

    if array(&body["body"]).is_empty() {
        context.emit("empty_branch", body, "This conditional branch is empty");
    }

    if kind(alternative) == "AstStatBlock" {
        if array(&alternative["body"]).is_empty() {
            context.emit(
                "empty_branch",
                alternative,
                "This conditional branch is empty",
            );
        }

        if context.same(body, alternative) {
            context.emit(
                "identical_branches",
                alternative,
                "These branches have identical bodies",
            );
        }

        if exits(body) {
            context.emit(
                "redundant_else",
                alternative,
                "The previous branch exits; else is unnecessary",
            );
        }

        if field(test, "op") == "Not" {
            context.emit(
                "negated_condition",
                test,
                "Exchange the branches to use a positive condition",
            );
        }
    }

    if alternative.is_null() && array(&body["body"]).len() == 1 {
        let inner = &body["body"][0];

        if kind(inner) == "AstStatIf" && inner["elsebody"].is_null() {
            context.emit(
                "nested_condition",
                value,
                "These nested conditions can be combined",
            );
        }
    }

    let mut next = alternative;

    while kind(next) == "AstStatIf" {
        if context.same(test, &next["condition"]) {
            context.emit(
                "repeated_condition",
                &next["condition"],
                "This condition was already tested",
            );
        }

        next = &next["elsebody"];
    }
}

pub(super) fn exits(value: &Value) -> bool {
    match kind(value) {
        "AstStatReturn" | "AstStatBreak" | "AstStatContinue" => true,
        "AstStatBlock" => array(&value["body"]).last().is_some_and(exits),
        "AstStatIf" => exits(&value["thenbody"]) && exits(&value["elsebody"]),
        "AstStatExpr" => call(&value["expr"], "error"),
        _ => false,
    }
}

pub(super) fn block(context: &mut Context<'_>, value: &Value) {
    let statements = array(&value["body"]);
    let mut functions = BTreeSet::new();

    for statement in statements {
        if matches!(kind(statement), "AstStatFunction" | "AstStatLocalFunction") {
            let name = if kind(statement) == "AstStatLocalFunction" {
                field(&statement["name"], "name").to_owned()
            } else {
                context.text(&statement["name"]).to_owned()
            };

            if !functions.insert(name.clone()) {
                context.emit(
                    "duplicate_function",
                    &statement["name"],
                    format!("Function {name} is declared twice"),
                );
            }

            if kind(statement) == "AstStatFunction" && kind(&statement["name"]) == "AstExprGlobal" {
                let used = context.nodes.iter().any(|node| {
                    kind(node.value) == "AstExprGlobal"
                        && field(node.value, "global") == name
                        && !std::ptr::eq(node.value, &raw const statement["name"])
                        && !context
                            .span(node.value)
                            .zip(context.span(statement))
                            .is_some_and(|(read, body)| {
                                body.start <= read.start && read.end <= body.end
                            })
                });

                if !used {
                    context.emit(
                        "unused_function",
                        &statement["name"],
                        format!("Function {name} is unused"),
                    );
                }
            }
        }
    }

    for pair in statements.windows(2) {
        let first = &pair[0];
        let second = &pair[1];

        if exits(first) {
            context.emit(
                "unreachable_code",
                second,
                "This statement follows an unconditional exit",
            );
        }

        if field(first, "location")
            .split(" - ")
            .nth(1)
            .and_then(|value| value.split(',').next())
            == field(second, "location").split(',').next()
        {
            context.emit(
                "multiple_statements",
                second,
                "Multiple statements occupy this line",
            );
        }

        if kind(first) == "AstStatAssign"
            && kind(second) == "AstStatAssign"
            && array(&first["vars"]).len() == 1
            && array(&first["values"]).len() == 1
            && array(&second["vars"]).len() == 1
            && array(&second["values"]).len() == 1
            && context.same(&first["vars"][0], &second["values"][0])
            && context.same(&first["values"][0], &second["vars"][0])
        {
            context.emit(
                "incomplete_swap",
                second,
                "The first assignment already overwrote the value needed here",
            );
        }

        clone_loop(context, first, second);
    }
}

fn clone_loop(context: &mut Context<'_>, first: &Value, second: &Value) {
    if kind(first) != "AstStatLocal"
        || array(&first["vars"]).len() != 1
        || array(&first["values"]).len() != 1
        || kind(&first["values"][0]) != "AstExprTable"
        || !array(&first["values"][0]["items"]).is_empty()
    {
        return;
    }

    if kind(second) != "AstStatForIn"
        || array(&second["vars"]).len() != 2
        || array(&second["values"]).len() != 1
        || !call(&second["values"][0], "pairs")
        || array(&second["body"]["body"]).len() != 1
    {
        return;
    }

    let assignment = &second["body"]["body"][0];

    if kind(assignment) != "AstStatAssign"
        || array(&assignment["vars"]).len() != 1
        || array(&assignment["values"]).len() != 1
    {
        return;
    }

    let target = &assignment["vars"][0];

    if kind(target) == "AstExprIndexExpr"
        && local(&target["expr"]) == Some(field(&first["vars"][0], "location"))
        && local(&target["index"]) == Some(field(&second["vars"][0], "location"))
        && local(&assignment["values"][0]) == Some(field(&second["vars"][1], "location"))
    {
        context.emit(
            "manual_table_clone",
            second,
            "This loop can copy the fresh table with table.clone",
        );
    }
}

fn shallow<'value>(value: &'value Value, output: &mut Vec<&'value Value>) {
    nested(value, output, false);
}

pub(super) fn nested<'value>(value: &'value Value, output: &mut Vec<&'value Value>, loops: bool) {
    if kind(value) == "AstExprFunction"
        || (!loops
            && matches!(
                kind(value),
                "AstStatWhile" | "AstStatRepeat" | "AstStatFor" | "AstStatForIn"
            ))
    {
        return;
    }

    match value {
        Value::Object(fields) => {
            if !kind(value).is_empty() {
                output.push(value);
            }

            for (name, child) in fields {
                if name != "local" {
                    nested(child, output, loops);
                }
            }
        }

        Value::Array(values) => {
            for child in values {
                nested(child, output, loops);
            }
        }

        _ => {}
    }
}

pub(super) fn loops(context: &mut Context<'_>, value: &Value) {
    let body = &value["body"];

    if array(&body["body"]).is_empty() {
        context.emit("empty_loop", body, "This loop body is empty");
    }

    condition(context, &value["condition"]);

    if kind(value) == "AstStatWhile" && truth(&value["condition"]) == Some(false)
        || kind(value) == "AstStatRepeat" && truth(&value["condition"]) == Some(true)
    {
        context.emit(
            "constant_condition",
            &value["condition"],
            "This loop condition is constant",
        );
    }

    if kind(value) == "AstStatFor" {
        if number(&value["step"]) == Some(0.0) {
            context.emit(
                "zero_loop_step",
                &value["step"],
                "The loop counter never advances",
            );
        }

        let descending = number(&value["from"])
            .zip(number(&value["to"]))
            .is_some_and(|(from, to)| from > to)
            || (field(&value["from"], "op") == "Len"
                && number(&value["to"]).is_some_and(|limit| limit <= 1.0));

        if descending
            && (value["step"].is_null() || number(&value["step"]).is_some_and(|step| step >= 0.0))
        {
            context.emit(
                "invalid_reverse_loop",
                value,
                "A descending loop needs a negative step",
            );
        }
    }

    let mut expressions = Vec::new();
    shallow(body, &mut expressions);

    for expression in expressions {
        if kind(expression) == "AstExprCall"
            && array(&expression["args"]).len() == 1
            && kind(&expression["args"][0]) == "AstExprConstantString"
            && (call(expression, "require")
                || (context.roblox
                    && field(&expression["func"], "index") == "GetService"
                    && global(&expression["func"]["expr"]).as_deref() == Some("game")))
        {
            context.emit(
                "loop_invariant_call",
                expression,
                "This lookup repeats with the same argument every iteration",
            );
        }

        if kind(expression) == "AstStatCompoundAssign"
            && field(expression, "op") == "Concat"
            && local(&expression["var"])
                .and_then(|identity| context.location(identity))
                .zip(context.span(value))
                .is_some_and(|(binding, body)| binding.start < body.start)
        {
            context.emit(
                "loop_string_concatenation",
                expression,
                "This loop repeatedly copies an accumulating string",
            );
        }

        if kind(expression) == "AstStatAssign" {
            for (target, assigned) in array(&expression["vars"])
                .iter()
                .zip(array(&expression["values"]))
            {
                if field(assigned, "op") == "Concat"
                    && (context.same(target, &assigned["left"])
                        || context.same(target, &assigned["right"]))
                    && local(target)
                        .and_then(|identity| context.location(identity))
                        .zip(context.span(value))
                        .is_some_and(|(binding, body)| binding.start < body.start)
                {
                    context.emit(
                        "loop_string_concatenation",
                        expression,
                        "This loop repeatedly copies an accumulating string",
                    );
                }
            }
        }
    }
}
