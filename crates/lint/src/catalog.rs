use std::sync::LazyLock;

pub(crate) const SYNTAX: &[&str] = &[
    "bad_string_escape",
    "compare_nan",
    "constant_condition",
    "constant_table_comparison",
    "length_as_condition",
    "zero_step_loop",
    "divide_by_zero",
    "empty_if",
    "empty_loop",
    "if_same_then_else",
    "mixed_table",
    "and_or_conditional",
    "collapsible_if",
    "else_after_return",
    "negated_condition",
    "parenthesized_conditions",
];

const NATIVE: &[&str] = &[
    "almost_swapped",
    "mismatched_argument_count",
    "must_use",
    "self_assignment",
    "ignored_pcall_result",
    "unscoped_variables",
    "roblox_incorrect_color3_new_bounds",
    "roblox_manual_fromscale_or_fromoffset",
    "roblox_prefer_get_players",
    "roblox_suspicious_udim2_new",
    "implicit_any_local",
    "implicit_any_parameter",
    "if_expression_assignment",
    "non_const_require",
    "global_usage",
    "type_check_inside_call",
    "loop_invariant_call",
    "manual_table_clone",
    "string_concat_in_loop",
    "unused_variable",
    "prefer_const",
    "deprecated",
    "restricted_globals",
    "restricted_module_paths",
];

const OPTIONAL: &[&str] = &[
    "implicit_any_local",
    "implicit_any_parameter",
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
    SYNTAX
        .iter()
        .chain(NATIVE)
        .map(|name| Definition {
            name: (*name).to_owned(),
            upstream: false,
            enabled: !OPTIONAL.contains(name),
            recommended: matches!(
                *name,
                "prefer_const" | "non_const_require" | "type_check_inside_call"
            ),
        })
        .chain(instar_luau::warnings().into_iter().map(|name| Definition {
            name,
            upstream: true,
            enabled: true,
            recommended: false,
        }))
        .collect()
});

pub(crate) struct Definition {
    pub(crate) name: String,
    pub(crate) upstream: bool,
    pub(crate) enabled: bool,
    pub(crate) recommended: bool,
}
