use std::sync::LazyLock;

const NAMES: &[&str] = &[
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
    "almost_swapped",
    "must_use",
    "self_assignment",
    "ignored_pcall_result",
    "unscoped_variables",
    "roblox_incorrect_color3_new_bounds",
    "roblox_manual_fromscale_or_fromoffset",
    "roblox_prefer_get_players",
    "roblox_suspicious_udim2_new",
    "if_expression_assignment",
    "non_const_require",
    "global_usage",
    "type_check_inside_call",
    "loop_invariant_call",
    "manual_table_clone",
    "string_concat_in_loop",
    "unused_variable",
    "prefer_const",
    "restricted_globals",
    "restricted_module_paths",
    "UnknownGlobal",
    "GlobalUsedAsLocal",
    "LocalShadow",
    "SameLineStatement",
    "MultiLineStatement",
    "LocalUnused",
    "FunctionUnused",
    "ImportUnused",
    "BuiltinGlobalWrite",
    "PlaceholderRead",
    "UnreachableCode",
    "UnknownType",
    "ForRange",
    "UnbalancedAssignment",
    "ImplicitReturn",
    "TableLiteral",
    "UninitializedLocal",
    "DuplicateFunction",
    "DuplicateCondition",
    "DuplicateLocal",
    "MisleadingAndOr",
    "IntegerParsing",
    "ComparisonPrecedence",
    "CommentDirective",
    "RedundantNativeAttribute",
];

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
            is_enabled: !OPTIONAL.contains(&name),
            is_recommended: matches!(
                name,
                "prefer_const" | "non_const_require" | "type_check_inside_call"
            ),
        })
        .collect()
});

pub(crate) struct Definition {
    pub(crate) name: String,
    pub(crate) is_enabled: bool,
    pub(crate) is_recommended: bool,
}

/// Return the supported lint rule names.
pub fn rules() -> impl Iterator<Item = &'static str> {
    NAMES.iter().copied()
}
