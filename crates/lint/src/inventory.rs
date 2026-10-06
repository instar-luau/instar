macro_rules! inventory {
    ($generate:ident) => {
        $generate! {
            almost_swapped => AlmostSwapped: Settings, "Assignments that appear to swap values incorrectly.";
            bad_string_escape => BadStringEscape: Settings, "Invalid or suspicious string escapes.";
            compare_nan => CompareNan: Settings, "Comparisons against NaN.";
            constant_condition => ConstantCondition: Settings, "Conditions with constant outcomes.";
            constant_table_comparison => ConstantTableComparison: Settings, "Constant comparisons of table values.";
            length_as_condition => LengthAsCondition: Settings, "Lengths used directly as conditions.";
            mismatched_argument_count => MismatchedArgumentCount: Settings, "Calls with mismatched argument counts.";
            must_use => MustUse: Settings, "Discarded results that must be used.";
            zero_step_loop => ZeroStepLoop: Settings, "Numeric loops with zero steps.";
            divide_by_zero => DivideByZero: Settings, "Division by zero.";
            self_assignment => SelfAssignment: Settings, "Assignments of a binding to itself.";
            empty_if => EmptyIf: Settings, "Empty conditional branches.";
            empty_loop => EmptyLoop: Settings, "Empty loop bodies.";
            if_same_then_else => IfSameThenElse: Settings, "Identical conditional branches.";
            ignored_pcall_result => IgnoredPcallResult: Settings, "Ignored protected-call results.";
            mixed_table => MixedTable: Settings, "Mixed array and dictionary table entries.";
            unscoped_variables => UnscopedVariables: Settings, "Variables declared without explicit scope.";
            roblox_incorrect_color3_new_bounds => RobloxIncorrectColor3NewBounds: Settings, "Out-of-range Color3.new arguments.";
            roblox_manual_fromscale_or_fromoffset => RobloxManualFromscaleOrFromoffset: Settings, "Manual equivalents of fromScale or fromOffset.";
            roblox_prefer_get_players => RobloxPreferGetPlayers: Settings, "Player enumeration better expressed with `GetPlayers`.";
            roblox_suspicious_udim2_new => RobloxSuspiciousUdim2New: Settings, "Suspicious UDim2.new arguments.";
            implicit_any_local => ImplicitAnyLocal: AllowedSettings, "Local bindings with implicit any types.";
            implicit_any_parameter => ImplicitAnyParameter: AllowedSettings, "Parameters with implicit any types.";
            and_or_conditional => AndOrConditional: AllowedSettings, "Conditional expressions written with and/or.";
            collapsible_if => CollapsibleIf: AllowedSettings, "Nested conditionals that can be collapsed.";
            else_after_return => ElseAfterReturn: AllowedSettings, "Else branches following returning branches.";
            if_expression_assignment => IfExpressionAssignment: AllowedSettings, "Assignments expressible with if expressions.";
            negated_condition => NegatedCondition: AllowedSettings, "Conditionals with negated conditions.";
            non_const_require => NonConstRequire: AllowedSettings, "Require calls with nonconstant arguments.";
            parenthesized_conditions => ParenthesizedConditions: AllowedSettings, "Unnecessary parentheses around conditions.";
            global_usage => GlobalUsage: AllowedSettings, "Uses of global bindings.";
            type_check_inside_call => TypeCheckInsideCall: AllowedSettings, "Type checks inside call expressions.";
            loop_invariant_call => LoopInvariantCall: AllowedSettings, "Loop-invariant calls inside loops.";
            manual_table_clone => ManualTableClone: AllowedSettings, "Manual table cloning.";
            string_concat_in_loop => StringConcatInLoop: AllowedSettings, "Repeated string concatenation inside loops.";
            unused_variable => UnusedVariable: UnusedVariable, "Unused local, parameter or loop binding.";
            high_cyclomatic_complexity => HighCyclomaticComplexity: HighCyclomaticComplexity, "Excessive function cyclomatic complexity.";
            prefer_const => PreferConst: PreferConst, "Immutable binding preference.";
            deprecated => Deprecated: Deprecated, "A configured deprecated symbol.";
            restricted_globals => RestrictedGlobals: RestrictedGlobals, "A configured restricted global.";
            restricted_module_paths => RestrictedModulePaths: RestrictedModulePaths, "A configured restricted require request.";
        }
    };
}

pub(crate) use inventory;
