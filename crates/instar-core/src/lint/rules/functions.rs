use super::super::syntax::{Context, array, field, kind};

use super::{
    control::{exits, nested},
    declarations::duplicates,
};

use serde_json::Value;

pub(super) fn function(context: &mut Context<'_>, value: &Value) {
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
