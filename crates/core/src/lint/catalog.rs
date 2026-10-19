use std::sync::LazyLock;

use crate::lint::{analysis, expression};

const OPTIONAL: &[&str] = &[
    "and_or_conditional",
    "collapsible_if",
    "else_after_return",
    "if_expression_assignment",
    "negated_condition",
    "non_const_require",
    "parenthesized_conditions",
    "global_usage",
    "type_check_inside_call",
    "loop_invariant_call",
    "manual_table_clone",
    "string_concat_in_loop",
    "prefer_const",
];

pub(crate) static RULES: LazyLock<Vec<Definition>> = LazyLock::new(|| {
    rules()
        .map(|name| Definition {
            name: name.to_owned(),
            enabled: !OPTIONAL.contains(&name),
            recommended: matches!(
                name,
                "prefer_const" | "non_const_require" | "type_check_inside_call"
            ),
        })
        .collect()
});

pub(crate) struct Definition {
    pub(crate) name: String,
    pub(crate) enabled: bool,
    pub(crate) recommended: bool,
}

/// Return the supported lint rule names.
pub fn rules() -> impl Iterator<Item = &'static str> {
    expression::RULES.iter().chain(analysis::RULES).copied()
}
