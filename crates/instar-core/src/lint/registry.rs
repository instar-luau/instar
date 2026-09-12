use super::configuration::Level;

pub struct Rule {
    pub name: &'static str,
    pub group: &'static str,
    pub level: Level,
    pub description: &'static str,
}

macro_rules! rules {
    ($($name:literal, $group:literal, $level:ident, $description:literal;)*) => {
        pub const RULES: &[Rule] = &[$(Rule { name: $name, group: $group, level: Level::$level, description: $description }),*];
    };
}

rules! {
    "incomplete_swap", "correctness", Warn, "Consecutive assignments overwrite a value instead of swapping values.";
    "not_a_number_comparison", "correctness", Warn, "A not-a-number value cannot be compared for equality.";
    "table_identity_comparison", "correctness", Warn, "A fresh table is compared by identity, not contents.";
    "division_by_zero", "suspicious", Warn, "Division by a literal zero.";
    "duplicate_table_key", "correctness", Deny, "A repeated table key overwrites an earlier value.";
    "repeated_condition", "correctness", Warn, "An elseif repeats an earlier condition.";
    "identical_branches", "suspicious", Warn, "Conditional branches have identical bodies.";
    "invalid_reverse_loop", "correctness", Warn, "A descending loop requires a negative step.";
    "misplaced_type_comparison", "correctness", Warn, "Compare the result of type(), not its argument.";
    "unbalanced_assignment", "correctness", Warn, "Assignment target and value counts differ.";
    "zero_loop_step", "correctness", Deny, "A numeric loop has a zero step.";
    "invalid_string_escape", "correctness", Warn, "A string contains an undefined escape.";
    "argument_count", "correctness", Warn, "A locally known function receives an incorrect argument count.";
    "discarded_return", "correctness", Warn, "A required return value is discarded.";
    "invalid_directive", "correctness", Warn, "An analysis directive is unknown or misplaced.";
    "builtin_assignment", "suspicious", Warn, "An assignment overwrites a standard global.";
    "comparison_precedence", "correctness", Warn, "Comparison operators are grouped misleadingly.";
    "duplicate_function", "correctness", Warn, "A function is declared twice in one scope.";
    "duplicate_binding", "correctness", Deny, "A declaration repeats a binding name.";
    "invalid_format_string", "correctness", Deny, "A format string contains an invalid conversion.";
    "inconsistent_return", "suspicious", Allow, "A function returns values on some paths and falls through on others.";
    "misleading_conditional", "correctness", Warn, "An and/or conditional cannot select its middle value as intended.";
    "number_literal_overflow", "correctness", Warn, "An integer literal exceeds the supported width.";
    "placeholder_read", "suspicious", Warn, "The discard binding is read.";
    "invalid_table_operation", "correctness", Warn, "A table operation has an invalid argument count or index.";
    "untyped_local", "suspicious", Warn, "A local has neither an initializer nor an annotation.";
    "untyped_parameter", "suspicious", Allow, "A function parameter has no annotation.";
    "uninitialized_local", "correctness", Warn, "An uninitialized local is never assigned.";
    "invalid_type_name", "correctness", Warn, "A type query is compared with an impossible type name.";
    "undefined_variable", "correctness", Deny, "A referenced name is not declared.";
    "global_assignment", "suspicious", Warn, "An assignment creates a global.";
    "unused_function", "correctness", Warn, "A declared function is unused.";
    "unused_variable", "correctness", Warn, "A declared binding is never read.";
    "unused_import", "correctness", Warn, "A required module binding is unused.";
    "shadowed_binding", "suspicious", Warn, "A declaration hides a binding still in scope.";
    "global_environment", "suspicious", Warn, "Code accesses the global environment table.";
    "deprecated_function", "style", Warn, "A deprecated function is called.";
    "restricted_import", "style", Warn, "A module path is restricted by the project.";
    "function_complexity", "complexity", Allow, "A function exceeds its cyclomatic complexity limit.";
    "manual_table_clone", "performance", Warn, "A table-copy loop can use table.clone.";
    "constant_binding", "style", Allow, "An eligible binding can use const.";
    "restricted_global", "style", Warn, "A global is restricted by the project.";
    "color_bounds", "roblox", Warn, "Color3.new receives channels above the unit scale.";
    "dimension_arguments", "roblox", Warn, "UDim2.new receives two arguments instead of four.";
    "dimension_constructor", "roblox", Warn, "A dimension constructor can use fromScale or fromOffset.";
    "constant_import", "style", Allow, "An eligible module binding can use const.";
    "unreachable_code", "correctness", Warn, "A statement follows an unconditional control-flow exit.";
    "self_assignment", "suspicious", Warn, "An assignment writes a value back to itself.";
    "loop_string_concatenation", "performance", Warn, "A loop repeatedly copies an accumulating string.";
    "loop_invariant_call", "performance", Warn, "A loop repeats a call with an invariant result.";
    "length_condition", "correctness", Deny, "A length is always truthy, including zero.";
    "shadowed_builtin", "suspicious", Warn, "A local hides a standard global.";
    "ignored_protected_call", "suspicious", Warn, "A protected call discards its status and results.";
    "empty_branch", "suspicious", Warn, "A conditional branch is empty.";
    "empty_loop", "suspicious", Warn, "A loop body is empty.";
    "mixed_table", "suspicious", Warn, "A table mixes positional and named entries.";
    "multiple_statements", "style", Allow, "Multiple statements occupy one line.";
    "parenthesized_condition", "style", Warn, "A condition has unnecessary parentheses.";
    "constant_condition", "correctness", Warn, "A literal condition decides control flow in advance.";
    "redundant_else", "style", Allow, "An else follows a branch that exits.";
    "nested_condition", "style", Allow, "A nested conditional can be collapsed.";
    "negated_condition", "style", Allow, "An if/else condition can be positive by exchanging branches.";
    "logical_conditional", "style", Allow, "An and/or value selection can be a conditional statement.";
    "conditional_expression", "style", Allow, "An if expression can be a conditional statement.";
}

#[must_use]
pub fn find(name: &str) -> Option<&'static Rule> {
    RULES.iter().find(|rule| rule.name == name)
}

pub(crate) fn upstream(name: &str) -> Option<&'static str> {
    Some(match name {
        "UnknownGlobal" => "undefined_variable",
        "DeprecatedGlobal" => "deprecated_function",
        "GlobalUsedAsLocal" => "global_assignment",
        "LocalShadow" => "shadowed_binding",
        "SameLineStatement" | "MultiLineStatement" => "multiple_statements",
        "LocalUnused" => "unused_variable",
        "FunctionUnused" => "unused_function",
        "ImportUnused" => "unused_import",
        "BuiltinGlobalWrite" => "builtin_assignment",
        "PlaceholderRead" => "placeholder_read",
        "UnreachableCode" => "unreachable_code",
        "UnknownType" => "invalid_type_name",
        "ForRange" => "invalid_reverse_loop",
        "UnbalancedAssignment" => "unbalanced_assignment",
        "ImplicitReturn" => "inconsistent_return",
        "DuplicateLocal" => "duplicate_binding",
        "DuplicateFunction" => "duplicate_function",
        "TableLiteral" => "duplicate_table_key",
        "TableOperations" => "invalid_table_operation",
        "MisleadingAndOr" => "misleading_conditional",
        "CommentDirective" => "invalid_directive",
        "IntegerParsing" => "number_literal_overflow",
        "ComparisonPrecedence" => "comparison_precedence",
        "FormatString" => "invalid_format_string",
        "UninitializedLocal" => "uninitialized_local",
        _ => return None,
    })
}
