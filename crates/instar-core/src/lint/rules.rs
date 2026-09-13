use super::{
    Edit,
    syntax::{
        Context, array, call, comparison, expands, field, global, kind, local, number, truth,
        unwrap,
    },
};

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn run(context: &mut Context<'_>) {
    let nodes = context
        .nodes
        .iter()
        .map(|node| (node.value, node.parent, node.scope))
        .collect::<Vec<_>>();

    let mut functions = BTreeMap::new();

    for (value, _, _) in &nodes {
        if kind(value) == "AstStatLocalFunction" {
            functions.insert(field(&value["name"], "location"), &value["func"]);
        }

        if kind(value) == "AstStatLocal"
            && array(&value["vars"]).len() == 1
            && array(&value["values"]).len() == 1
            && kind(&value["values"][0]) == "AstExprFunction"
        {
            functions.insert(field(&value["vars"][0], "location"), &value["values"][0]);
        }
    }

    for (value, parent, scope) in nodes {
        match kind(value) {
            "AstStatBlock" => block(context, value),
            "AstStatIf" => conditional(context, value),

            "AstStatWhile" | "AstStatRepeat" | "AstStatFor" | "AstStatForIn" => {
                loops(context, value);
            }

            "AstStatLocal" => declaration(context, value),
            "AstStatAssign" => assignment(context, value),
            "AstLocal" => binding(context, value, parent, scope),
            "AstExprGlobal" => globals(context, value, parent),

            "AstExprLocal"
                if field(&value["local"], "name") == "_"
                    && context
                        .reads
                        .get(field(&value["local"], "location"))
                        .is_some_and(|reads| {
                            reads.iter().any(|read| std::ptr::eq(*read, value))
                        }) =>
            {
                context.emit("placeholder_read", value, "The discard binding is read");
            }

            "AstExprBinary" => binary(context, value),
            "AstExprTable" => table(context, value),
            "AstExprCall" => invocation(context, value, parent, &functions),
            "AstExprConstantNumber" => literal(context, value),
            "AstExprConstantString" => escapes(context, value),
            "AstExprFunction" => function(context, value),

            "AstExprIfElse" => context.emit(
                "conditional_expression",
                value,
                "Use a conditional statement for value selection",
            ),

            _ => {}
        }
    }

    directives(context);
}

fn binding(context: &mut Context<'_>, value: &Value, parent: &Value, scope: &Value) {
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

fn declaration(context: &mut Context<'_>, value: &Value) {
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
        && context.settings.level("constant_import") != super::configuration::Level::Allow
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
            vec![Edit {
                start: range.start,
                end: range.start + "local".len(),
                text: "const".into(),
            }],
        );
    }
}

fn duplicates(context: &mut Context<'_>, bindings: &[Value]) {
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
    if targets.len() != values.len() && !values.last().is_some_and(expands) {
        context.emit(
            "unbalanced_assignment",
            node,
            format!("{} targets receive {} values", targets.len(), values.len()),
        );
    }
}

fn assignment(context: &mut Context<'_>, value: &Value) {
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

fn globals(context: &mut Context<'_>, value: &Value, parent: &Value) {
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

fn table(context: &mut Context<'_>, value: &Value) {
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

fn binary(context: &mut Context<'_>, value: &Value) {
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

        if truth(&left["right"]) == Some(false) || comparison(&left["right"]) {
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

fn condition(context: &mut Context<'_>, value: &Value) {
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

fn conditional(context: &mut Context<'_>, value: &Value) {
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

fn exits(value: &Value) -> bool {
    match kind(value) {
        "AstStatReturn" | "AstStatBreak" | "AstStatContinue" => true,
        "AstStatBlock" => array(&value["body"]).last().is_some_and(exits),
        "AstStatIf" => exits(&value["thenbody"]) && exits(&value["elsebody"]),
        "AstStatExpr" => call(&value["expr"], "error"),
        _ => false,
    }
}

fn block(context: &mut Context<'_>, value: &Value) {
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

fn nested<'value>(value: &'value Value, output: &mut Vec<&'value Value>, loops: bool) {
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

fn loops(context: &mut Context<'_>, value: &Value) {
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

fn escapes(context: &mut Context<'_>, value: &Value) {
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

fn literal(context: &mut Context<'_>, value: &Value) {
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

fn function(context: &mut Context<'_>, value: &Value) {
    duplicates(context, array(&value["args"]));
    let mut nodes = Vec::new();
    nested(&value["body"], &mut nodes, true);

    let score = 1 + nodes
        .iter()
        .filter(|node| {
            matches!(
                kind(node),
                "AstStatIf" | "AstStatWhile" | "AstStatRepeat" | "AstStatFor" | "AstStatForIn"
            ) || (kind(node) == "AstExprBinary" && matches!(field(node, "op"), "And" | "Or"))
        })
        .count();

    if score
        > context
            .settings
            .options
            .function_complexity
            .maximum_complexity as usize
    {
        context.emit(
            "function_complexity",
            value,
            format!("Function complexity is {score}"),
        );
    }

    if nodes
        .iter()
        .any(|node| kind(node) == "AstStatReturn" && !array(&node["list"]).is_empty())
        && !exits(&value["body"])
    {
        context.emit(
            "inconsistent_return",
            value,
            "This function returns values on some paths and falls through on others",
        );
    }
}

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

fn invocation(
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
    roblox(context, value, &path);
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

fn roblox(context: &mut Context<'_>, value: &Value, path: &str) {
    if !context.roblox {
        return;
    }

    let arguments = array(&value["args"]);

    if path == "Color3.new"
        && arguments
            .iter()
            .filter_map(number)
            .any(|channel| channel > 1.0)
    {
        context.emit(
            "color_bounds",
            value,
            "Color3.new channels use the unit scale",
        );
    }

    if path != "UDim2.new" {
        return;
    }

    if arguments.len() == 2 {
        context.emit(
            "dimension_arguments",
            value,
            "UDim2.new takes four arguments",
        );
    }

    if arguments.len() == 4 {
        if number(&arguments[1]) == Some(0.0) && number(&arguments[3]) == Some(0.0) {
            context.fix(
                "dimension_constructor",
                value,
                format!(
                    "UDim2.fromScale({}, {})",
                    context.text(&arguments[0]),
                    context.text(&arguments[2])
                ),
            );
        } else if number(&arguments[0]) == Some(0.0) && number(&arguments[2]) == Some(0.0) {
            context.fix(
                "dimension_constructor",
                value,
                format!(
                    "UDim2.fromOffset({}, {})",
                    context.text(&arguments[1]),
                    context.text(&arguments[3])
                ),
            );
        }
    }
}

fn directives(context: &mut Context<'_>) {
    for range in context.comments.clone() {
        let text = context
            .source
            .text()
            .ok()
            .and_then(|text| text.get(range.clone()))
            .unwrap_or("");

        if let Some(directive) = text.strip_prefix("--!") {
            let directive = directive.trim();
            let name = directive.split_whitespace().next().unwrap_or("");

            let known = matches!(
                name,
                "strict" | "nonstrict" | "nocheck" | "nolint" | "native" | "optimize"
            );

            let misplaced = context.nodes.iter().any(|node| {
                kind(node.value).starts_with("AstStat")
                    && kind(node.value) != "AstStatBlock"
                    && context
                        .span(node.value)
                        .is_some_and(|statement| statement.start < range.start)
            });

            if !known || misplaced {
                context.emit_range(
                    "invalid_directive",
                    range,
                    "Unknown or misplaced analysis directive",
                    Vec::new(),
                );
            }
        }
    }
}
