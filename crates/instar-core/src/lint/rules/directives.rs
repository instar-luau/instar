use super::super::syntax::{Context, kind};

pub(super) fn apply(context: &mut Context<'_>) {
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
