mod bindings;
mod calls;
mod control;
mod declarations;
mod directives;
mod expressions;
mod functions;
mod roblox;

use super::syntax::{Context, array, field, kind};
use std::collections::BTreeMap;

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
            "AstStatBlock" => control::block(context, value),
            "AstStatIf" => control::conditional(context, value),

            "AstStatWhile" | "AstStatRepeat" | "AstStatFor" | "AstStatForIn" => {
                control::loops(context, value);
            }

            "AstStatLocal" => declarations::declaration(context, value),
            "AstStatAssign" => declarations::assignment(context, value),
            "AstLocal" => bindings::binding(context, value, parent, scope),
            "AstExprGlobal" => declarations::globals(context, value, parent),

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

            "AstExprBinary" => expressions::binary(context, value),
            "AstExprTable" => expressions::table(context, value),
            "AstExprCall" => calls::invocation(context, value, parent, &functions),
            "AstExprConstantNumber" => expressions::literal(context, value),
            "AstExprConstantString" => expressions::escapes(context, value),
            "AstExprFunction" => functions::function(context, value),

            "AstExprIfElse" => context.emit(
                "conditional_expression",
                value,
                "Use a conditional statement for value selection",
            ),

            _ => {}
        }
    }

    directives::apply(context);
}
