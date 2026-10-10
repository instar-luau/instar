use std::{fmt, sync::LazyLock};

use instar_luau::{SemanticCategory, UpstreamWarningIdentity};
use instar_shared::Options;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    RuleInput, RuleResult, RuleSettings,
    rules::{assignments, bindings, calls, control, expressions, roblox},
};

const SYNTAX: Prerequisites = Prerequisites {
    syntax: true,
    bindings: false,
    environment: false,
    semantics: &[],
};

const LEXICAL: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: false,
    semantics: &[],
};

const ENVIRONMENT: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[],
};

const INFERRED: Prerequisites = Prerequisites {
    syntax: true,
    bindings: false,
    environment: true,
    semantics: &[SemanticCategory::Types],
};

const CALLS: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[SemanticCategory::Calls],
};

const TYPES: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[SemanticCategory::Bindings, SemanticCategory::Types],
};

const EFFECTS: Prerequisites = Prerequisites {
    syntax: true,
    bindings: true,
    environment: true,
    semantics: &[
        SemanticCategory::Calls,
        SemanticCategory::Effects,
        SemanticCategory::Dependencies,
        SemanticCategory::Occurrences,
    ],
};

macro_rules! inventory {
    ($consumer:ident) => {
        $consumer! {
            AlmostSwapped, "almost_swapped", BasicSettings, Basic,
                "Assignments that overwrite a value before swapping it.",
                "Assignments that overwrite a value before swapping it.",
                "Assignments that overwrite a value before swapping it. Example: `a=b; b=a`.",
                Assignments, LEXICAL, None, Instar(assignments::evaluate);
            BadStringEscape, "bad_string_escape", BasicSettings, Basic,
                "Invalid or suspicious string escapes.",
                "Invalid or suspicious string escapes.",
                "Invalid or suspicious string escapes. Example: `\"\\\\q\"`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            CompareNan, "compare_nan", BasicSettings, Basic,
                "Comparisons against NaN have fixed outcomes.",
                "Comparisons against NaN have fixed outcomes.",
                "Comparisons against NaN have fixed outcomes. Example: `value == 0/0`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            ConstantCondition, "constant_condition", BasicSettings, Basic,
                "Conditions with statically known truth values.",
                "Conditions with statically known truth values.",
                "Conditions with statically known truth values. Example: `if true then end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            ConstantTableComparison, "constant_table_comparison", BasicSettings, Basic,
                "Fresh table values compared by identity.",
                "Fresh table values compared by identity.",
                "Fresh table values compared by identity. Example: `{} == {}`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            LengthAsCondition, "length_as_condition", BasicSettings, Basic,
                "Lengths used as booleans even though zero is truthy.",
                "Lengths used as booleans even though zero is truthy.",
                "Lengths used as booleans even though zero is truthy. Example: `if #items then end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            MismatchedArgumentCount, "mismatched_argument_count", BasicSettings, Basic,
                "Calls with incompatible known lexical or imported argument constraints.",
                "Calls with incompatible known argument constraints.",
                "Calls with incompatible lexical or imported argument constraints. Example: `local function f(a) end; f()`.",
                Calls, CALLS, None, Instar(calls::evaluate);
            MustUse, "must_use", BasicSettings, Basic,
                "Discarded results from known result-producing operations.",
                "Discarded results from known result-producing operations.",
                "Discarded results from known result-producing operations. Example: `math.abs(-1)`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            ZeroStepLoop, "zero_step_loop", BasicSettings, Basic,
                "Numeric loops with a statically zero step.",
                "Numeric loops with a statically zero step.",
                "Numeric loops with a statically zero step. Example: `for i=1,4,0 do end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            DivideByZero, "divide_by_zero", BasicSettings, Basic,
                "Statically identifiable division or modulo by zero.",
                "Statically identifiable division or modulo by zero.",
                "Statically identifiable division or modulo by zero. Example: `return 1/0`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            SelfAssignment, "self_assignment", BasicSettings, Basic,
                "Assignments of a binding to itself.",
                "Assignments of a binding to itself.",
                "Assignments of a binding to itself. Example: `a=a`.",
                Assignments, LEXICAL, Bindings, Instar(assignments::evaluate);
            EmptyIf, "empty_if", BasicSettings, Basic,
                "Conditional branches with empty bodies.",
                "Conditional branches with empty bodies.",
                "Conditional branches with empty bodies. Example: `if flag then end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            EmptyLoop, "empty_loop", BasicSettings, Basic,
                "Empty loops that cannot make progress.",
                "Empty loops that cannot make progress.",
                "Empty loops that cannot make progress. Example: `while flag do end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IfSameThenElse, "if_same_then_else", BasicSettings, Basic,
                "Conditional branches with identical bodies.",
                "Conditional branches with identical bodies.",
                "Conditional branches with identical bodies. Example: `if flag then f() else f() end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IgnoredPcallResult, "ignored_pcall_result", BasicSettings, Basic,
                "Protected-call success results that are ignored.",
                "Protected-call success results that are ignored.",
                "Protected-call success results that are ignored. Example: `pcall(work)`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            MixedTable, "mixed_table", BasicSettings, Basic,
                "Table literals mixing positional and keyed entries.",
                "Table literals mixing positional and keyed entries.",
                "Table literals mixing positional and keyed entries. Example: `{1,key=2}`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            UnscopedVariables, "unscoped_variables", BasicSettings, Basic,
                "Assignments introducing undeclared global bindings.",
                "Assignments introducing undeclared global bindings.",
                "Assignments introducing undeclared global bindings. Example: `value=1`.",
                Bindings, ENVIRONMENT, None, Instar(bindings::evaluate);
            RobloxIncorrectColor3NewBounds, "roblox_incorrect_color3_new_bounds", BasicSettings, Basic,
                "`Color3.new` components outside zero through one.",
                "Color3.new components outside zero through one.",
                "Color3.new components outside zero through one. Example: `Color3.new(255,0,0)`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxManualFromscaleOrFromoffset, "roblox_manual_fromscale_or_fromoffset", BasicSettings, Basic,
                "`UDim2.new` calls expressible as `fromScale` or `fromOffset`.",
                "UDim2.new calls expressible as fromScale or fromOffset.",
                "UDim2.new calls expressible as fromScale or fromOffset. Example: `UDim2.new(1,0,1,0)`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxPreferGetPlayers, "roblox_prefer_get_players", BasicSettings, Basic,
                "Player enumeration better expressed with `GetPlayers`.",
                "Player enumeration better expressed with GetPlayers.",
                "Player enumeration better expressed with GetPlayers. Example: `game:GetService(\"Players\"):GetChildren()`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxSuspiciousUdim2New, "roblox_suspicious_udim2_new", BasicSettings, Basic,
                "`UDim2.new` calls with suspicious scale and offset components.",
                "UDim2.new calls with suspicious scale and offset components.",
                "UDim2.new calls with suspicious scale and offset components. Example: `UDim2.new(1,2)`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            ImplicitAnyLocal, "implicit_any_local", BasicSettings, Basic,
                "Unannotated local bindings with known inferred any types.",
                "Unannotated local bindings with known inferred any types.",
                "Unannotated local bindings with known inferred any types. Example: `local value=unknown`.",
                Bindings, TYPES, None, Instar(bindings::evaluate);
            ImplicitAnyParameter, "implicit_any_parameter", BasicSettings, Basic,
                "Unannotated parameters with known inferred any types.",
                "Unannotated parameters with known inferred any types.",
                "Unannotated parameters with known inferred any types. Example: `local function f(value) end`.",
                Bindings, TYPES, None, Instar(bindings::evaluate);
            AndOrConditional, "and_or_conditional", BasicSettings, Basic,
                "Conditional and/or expressions better expressed with if.",
                "Conditional and/or expressions better expressed with if.",
                "Conditional and/or expressions better expressed with if. Example: `condition and left or right`.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            CollapsibleIf, "collapsible_if", BasicSettings, Basic,
                "Nested single-branch conditionals that can be combined.",
                "Nested single-branch conditionals that can be combined.",
                "Nested single-branch conditionals that can be combined. Example: `if a then if b then work() end end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            ElseAfterReturn, "else_after_return", BasicSettings, Basic,
                "Else branches following branches that always return.",
                "Else branches following branches that always return.",
                "Else branches following branches that always return. Example: `if a then return else work() end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IfExpressionAssignment, "if_expression_assignment", BasicSettings, Basic,
                "Matching assignments expressible as conditional expressions.",
                "Matching assignments expressible as conditional expressions.",
                "Matching assignments expressible as conditional expressions. Example: `if a then x=1 else x=2 end`.",
                Control, LEXICAL, None, Instar(control::evaluate);
            NegatedCondition, "negated_condition", BasicSettings, Basic,
                "Negated conditions expressible by reversing branches.",
                "Negated conditions expressible by reversing branches.",
                "Negated conditions expressible by reversing branches. Example: `if not a then left() else right() end`.",
                Control, SYNTAX, None, Instar(control::evaluate);
            NonConstRequire, "non_const_require", BasicSettings, Basic,
                "Require calls with nonconstant arguments.",
                "Require calls with nonconstant arguments.",
                "Require calls with nonconstant arguments. Example: `require(path)`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            ParenthesizedConditions, "parenthesized_conditions", BasicSettings, Basic,
                "Unnecessary parentheses around conditions.",
                "Unnecessary parentheses around conditions.",
                "Unnecessary parentheses around conditions. Example: `if (flag) then end`.",
                Control, SYNTAX, Syntax, Instar(control::evaluate);
            GlobalUsage, "global_usage", BasicSettings, Basic,
                "References to global bindings.",
                "References to global bindings.",
                "References to global bindings. Example: `return global`.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            TypeCheckInsideCall, "type_check_inside_call", BasicSettings, Basic,
                "Comparisons misplaced inside type or typeof calls.",
                "Comparisons misplaced inside type or typeof calls.",
                "Comparisons misplaced inside type or typeof calls. Example: `type(value==\"number\")`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            LoopInvariantCall, "loop_invariant_call", BasicSettings, Basic,
                "Calls proven safe and independent of loop effects.",
                "Calls proven safe and independent of loop effects.",
                "Calls proven safe and independent of loop effects. Example: `while flag do math.abs(1) end`.",
                Calls, EFFECTS, None, Instar(calls::evaluate);
            ManualTableClone, "manual_table_clone", BasicSettings, Basic,
                "Table-copy loops expressible with table.clone.",
                "Table-copy loops expressible with table.clone.",
                "Table-copy loops expressible with table.clone. Example: `for key,value in pairs(source) do copy[key]=value end`.",
                Expressions, LEXICAL, None, Instar(expressions::evaluate);
            StringConcatInLoop, "string_concat_in_loop", BasicSettings, Basic,
                "Repeated string concatenation inside loops.",
                "Repeated string concatenation inside loops.",
                "Repeated string concatenation inside loops. Example: `while flag do text ..= item end`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            UnusedVariable, "unused_variable", UnusedBindings, UnusedBindings,
                "Unread local bindings, parameters, and loop variables.",
                "Unread local bindings, parameters, and loop variables.",
                "Unread local bindings, parameters, and loop variables. Example: `local unused=1`.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            PreferConst, "prefer_const", ImmutableBindings, ImmutableBindings,
                "Initialized local bindings that are never reassigned.",
                "Initialized local bindings that are never reassigned.",
                "Initialized local bindings that are never reassigned. Example: `local fixed=1; return fixed`.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            Deprecated, "deprecated", DeprecatedSymbols, DeprecatedSymbols,
                "Project-deprecated symbol paths with replacements.",
                "Project-deprecated symbol paths with replacements.",
                "Project-deprecated symbol paths with replacements. Example: `old.api()`.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            RestrictedGlobals, "restricted_globals", RestrictedGlobals, RestrictedGlobals,
                "Uses of project-restricted global names.",
                "Uses of project-restricted global names.",
                "Uses of project-restricted global names. Example: `print(value)`.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            RestrictedModulePaths, "restricted_module_paths", RestrictedImports, RestrictedImports,
                "Requires matching project-restricted module paths.",
                "Requires matching project-restricted module paths.",
                "Requires matching project-restricted module paths. Example: `require(\"./blocked\")`.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            UnknownGlobal, "UnknownGlobal", BasicSettings, Basic,
                "Unknown global references.",
                "Unknown global references.",
                "Unknown global references. Example: `return missing`. Luau produces this warning as `UnknownGlobal`.",
                Bindings, ENVIRONMENT, None, Upstream;
            DeprecatedGlobal, "DeprecatedGlobal", BasicSettings, Basic,
                "Deprecated global references.",
                "Deprecated global references.",
                "Deprecated global references. Example: `collectgarbage()`. Luau produces this warning as `DeprecatedGlobal`.",
                Bindings, ENVIRONMENT, None, Upstream;
            GlobalUsedAsLocal, "GlobalUsedAsLocal", BasicSettings, Basic,
                "Global bindings better expressed as locals.",
                "Global bindings better expressed as locals.",
                "Global bindings better expressed as locals. Example: `state=0; state+=1`. Luau produces this warning as `GlobalUsedAsLocal`.",
                Bindings, ENVIRONMENT, None, Upstream;
            LocalShadow, "LocalShadow", BasicSettings, Basic,
                "Local declarations shadowing existing bindings.",
                "Local declarations shadowing existing bindings.",
                "Local declarations shadowing existing bindings. Example: `local value=1; do local value=2 end`. Luau produces this warning as `LocalShadow`.",
                Bindings, ENVIRONMENT, None, Upstream;
            SameLineStatement, "SameLineStatement", BasicSettings, Basic,
                "Multiple statements on one line.",
                "Multiple statements on one line.",
                "Multiple statements on one line. Example: `print(1) print(2)`. Luau produces this warning as `SameLineStatement`.",
                Control, SYNTAX, None, Upstream;
            MultiLineStatement, "MultiLineStatement", BasicSettings, Basic,
                "Misleading multiline statement layout.",
                "Misleading multiline statement layout.",
                "Misleading multiline statement layout. Example: `local value = work\\n(1)`. Luau produces this warning as `MultiLineStatement`.",
                Control, SYNTAX, None, Upstream;
            LocalUnused, "LocalUnused", BasicSettings, Basic,
                "Unused local bindings.",
                "Unused local bindings.",
                "Unused local bindings. Example: `local unused=1`. Luau produces this warning as `LocalUnused`.",
                Bindings, LEXICAL, None, Upstream;
            FunctionUnused, "FunctionUnused", BasicSettings, Basic,
                "Unused local functions.",
                "Unused local functions.",
                "Unused local functions. Example: `local function unused() end`. Luau produces this warning as `FunctionUnused`.",
                Bindings, LEXICAL, None, Upstream;
            ImportUnused, "ImportUnused", BasicSettings, Basic,
                "Unused imports.",
                "Unused imports.",
                "Unused imports. Example: `local unused=require(\"./module\")`. Luau produces this warning as `ImportUnused`.",
                Bindings, LEXICAL, None, Upstream;
            BuiltinGlobalWrite, "BuiltinGlobalWrite", BasicSettings, Basic,
                "Writes to builtin globals.",
                "Writes to builtin globals.",
                "Writes to builtin globals. Example: `print=function() end`. Luau produces this warning as `BuiltinGlobalWrite`.",
                Bindings, ENVIRONMENT, None, Upstream;
            PlaceholderRead, "PlaceholderRead", BasicSettings, Basic,
                "Reads from placeholder bindings.",
                "Reads from placeholder bindings.",
                "Reads from placeholder bindings. Example: `local _=1; return _`. Luau produces this warning as `PlaceholderRead`.",
                Bindings, ENVIRONMENT, None, Upstream;
            UnreachableCode, "UnreachableCode", BasicSettings, Basic,
                "Unreachable code.",
                "Unreachable code.",
                "Unreachable code. Example: `return 1; print(2)`. Luau produces this warning as `UnreachableCode`.",
                Control, SYNTAX, None, Upstream;
            UnknownType, "UnknownType", BasicSettings, Basic,
                "Unsupported names in runtime type tests.",
                "Unsupported names in runtime type tests.",
                "Unsupported names in runtime type tests. Example: `type(value)==\"integer\"`. Luau produces this warning as `UnknownType`.",
                Calls, SYNTAX, None, Upstream;
            ForRange, "ForRange", BasicSettings, Basic,
                "Suspicious numeric loop ranges.",
                "Suspicious numeric loop ranges.",
                "Suspicious numeric loop ranges. Example: `for index=10,1 do end`. Luau produces this warning as `ForRange`.",
                Control, SYNTAX, None, Upstream;
            UnbalancedAssignment, "UnbalancedAssignment", BasicSettings, Basic,
                "Assignment target and value mismatches.",
                "Assignment target and value mismatches.",
                "Assignment target and value mismatches. Example: `local left,right=1`. Luau produces this warning as `UnbalancedAssignment`.",
                Assignments, SYNTAX, None, Upstream;
            ImplicitReturn, "ImplicitReturn", BasicSettings, Basic,
                "Inconsistent explicit and implicit returns.",
                "Inconsistent explicit and implicit returns.",
                "Inconsistent explicit and implicit returns. Example: `local function f(flag) if flag then return 1 end end`. Luau produces this warning as `ImplicitReturn`.",
                Control, SYNTAX, None, Upstream;
            DuplicateLocal, "DuplicateLocal", BasicSettings, Basic,
                "Duplicate local declarations.",
                "Duplicate local declarations.",
                "Duplicate local declarations. Example: `local value,value=1,2`. Luau produces this warning as `DuplicateLocal`.",
                Bindings, LEXICAL, None, Upstream;
            FormatString, "FormatString", BasicSettings, Basic,
                "Invalid format strings or arguments.",
                "Invalid format strings or arguments.",
                "Invalid format strings or arguments. Example: `string.format(\"%d\", \"text\")`. Luau produces this warning as `FormatString`.",
                Calls, SYNTAX, None, Upstream;
            TableLiteral, "TableLiteral", BasicSettings, Basic,
                "Suspicious or duplicate table entries.",
                "Suspicious or duplicate table entries.",
                "Suspicious or duplicate table entries. Example: `{key=1,key=2}`. Luau produces this warning as `TableLiteral`.",
                Expressions, INFERRED, None, Upstream;
            UninitializedLocal, "UninitializedLocal", BasicSettings, Basic,
                "Reads before initialization.",
                "Reads before initialization.",
                "Reads before initialization. Example: `local value; return value`. Luau produces this warning as `UninitializedLocal`.",
                Bindings, LEXICAL, None, Upstream;
            DuplicateFunction, "DuplicateFunction", BasicSettings, Basic,
                "Duplicate function definitions.",
                "Duplicate function definitions.",
                "Duplicate function definitions. Example: `function object:work() end; function object:work() end`. Luau produces this warning as `DuplicateFunction`.",
                Bindings, LEXICAL, None, Upstream;
            DeprecatedApi, "DeprecatedApi", BasicSettings, Basic,
                "Deprecated API references.",
                "Deprecated API references.",
                "Deprecated API references. Example: `getfenv(1)`. Luau produces this warning as `DeprecatedApi`.",
                Calls, INFERRED, None, Upstream;
            TableOperations, "TableOperations", BasicSettings, Basic,
                "Suspicious table operations.",
                "Suspicious table operations.",
                "Suspicious table operations. Example: `table.insert(items, #items, value)`. Luau produces this warning as `TableOperations`.",
                Calls, INFERRED, None, Upstream;
            DuplicateCondition, "DuplicateCondition", BasicSettings, Basic,
                "Repeated conditional tests.",
                "Repeated conditional tests.",
                "Repeated conditional tests. Example: `if flag then left() elseif flag then right() end`. Luau produces this warning as `DuplicateCondition`.",
                Control, SYNTAX, None, Upstream;
            MisleadingAndOr, "MisleadingAndOr", BasicSettings, Basic,
                "Misleading conditional and/or results.",
                "Misleading conditional and/or results.",
                "Misleading conditional and/or results. Example: `flag and false or fallback`. Luau produces this warning as `MisleadingAndOr`.",
                Expressions, SYNTAX, None, Upstream;
            CommentDirective, "CommentDirective", BasicSettings, Basic,
                "Invalid analysis directives.",
                "Invalid analysis directives.",
                "Invalid analysis directives. Example: `--!unknown`. Luau produces this warning as `CommentDirective`.",
                Control, SYNTAX, None, Upstream;
            IntegerParsing, "IntegerParsing", BasicSettings, Basic,
                "Unexpected integer literal representation.",
                "Unexpected integer literal representation.",
                "Unexpected integer literal representation. Example: `9007199254740993`. Luau produces this warning as `IntegerParsing`.",
                Expressions, SYNTAX, None, Upstream;
            ComparisonPrecedence, "ComparisonPrecedence", BasicSettings, Basic,
                "Misleading comparison precedence.",
                "Misleading comparison precedence.",
                "Misleading comparison precedence. Example: `not left == right`. Luau produces this warning as `ComparisonPrecedence`.",
                Expressions, SYNTAX, None, Upstream;
            RedundantNativeAttribute, "RedundantNativeAttribute", BasicSettings, Basic,
                "Redundant native compilation annotations.",
                "Redundant native compilation annotations.",
                "Redundant native compilation annotations. Example: `--!native\\n@native local function work() end`. Luau produces this warning as `RedundantNativeAttribute`.",
                Control, SYNTAX, None, Upstream;
        }
    };
}

pub(crate) use inventory;

macro_rules! define_catalog {
    ($(
        $identity:ident, $name:literal, $parameters:ty, $schema:ident,
        $summary:literal, $description:literal, $documentation:literal,
        $category:ident, $prerequisites:expr, $fixes:ident, $producer:ident $(($evaluator:path))?;
    )*) => {
        static CATALOG: LazyLock<Vec<RuleDefinition>> = LazyLock::new(|| vec![$(
            RuleDefinition {
                identity: RuleIdentity::$identity,
                category: RuleCategory::$category,
                description: $description,
                documentation: $documentation,
                prerequisites: $prerequisites,
                settings: RuleSchema::$schema,
                fixes: FixSupport::$fixes,
                producer: define_catalog!(@producer $name, $producer $(($evaluator))?),
            },
        )*]);

        /// Stable public identities of supported lint rules.
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize, JsonSchema,
        )]
        pub enum RuleIdentity {
            $(
                #[doc = $summary]
                #[serde(rename = $name)]
                $identity,
            )*
        }

        impl RuleIdentity {
            /// Returns the canonical configuration and diagnostic name.
            #[must_use]
            pub const fn name(self) -> &'static str {
                match self { $(Self::$identity => $name,)* }
            }

            /// Resolves a canonical name without aliases.
            #[must_use]
            pub fn named(name: &str) -> Option<Self> {
                match name { $($name => Some(Self::$identity),)* _ => None }
            }
        }
    };
    (@producer $name:literal, Instar($evaluator:path)) => {
        RuleProducer::Instar($evaluator)
    };
    (@producer $name:literal, Upstream) => {
        RuleProducer::Upstream(UpstreamWarningIdentity { name: $name.to_owned() })
    };
}

inventory!(define_catalog);

impl fmt::Display for RuleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Evidence requested by a rule's producer.
#[derive(Clone, Copy, Debug)]
pub struct Prerequisites {
    /// Requires the captured lossless syntax.
    pub syntax: bool,

    /// Requires lexical declarations, references, and writes.
    pub bindings: bool,

    /// Requires the caller-established language environment.
    pub environment: bool,

    /// Required categories of detached semantic evidence.
    pub semantics: &'static [SemanticCategory],
}

/// Settings shape accepted by one rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuleSchema {
    /// No rule-specific parameters.
    Basic,

    /// Unread binding exemptions.
    UnusedBindings,

    /// Immutable declaration policy.
    ImmutableBindings,

    /// Deprecated paths and replacements.
    DeprecatedSymbols,

    /// Restricted global names and reasons.
    RestrictedGlobals,

    /// Restricted import paths and reasons.
    RestrictedImports,
}

/// Supported correction guarantees.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixSupport {
    /// No correction is currently supported.
    None,

    /// Supports syntax-preserving proposals.
    Syntax,

    /// Supports lexical-identity-preserving proposals.
    Bindings,
}

/// The sole producer of a catalog rule's findings.
#[derive(Clone)]
pub enum RuleProducer {
    /// Supplied-evidence Instar evaluator.
    Instar(fn(&RuleInput<'_, '_>, &RuleSettings, &Options) -> RuleResult),

    /// Native Luau warning identity; no duplicate Rust evaluator.
    Upstream(UpstreamWarningIdentity),
}

/// Responsibility category used independently of severity and producer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RuleCategory {
    /// Binding declarations and references.
    Bindings,

    /// Assignment sequences and targets.
    Assignments,

    /// Value expressions and table construction.
    Expressions,

    /// Conditions, branches, and loops.
    Control,

    /// Calls, results, and module imports.
    Calls,

    /// Roblox-specific construction and enumeration.
    Roblox,
}

/// A supported rule and its public policy contract.
pub struct RuleDefinition {
    /// Canonical public identity.
    pub identity: RuleIdentity,

    /// Responsibility used for category selection.
    pub category: RuleCategory,

    /// Short catalog description.
    pub description: &'static str,

    /// Explanation and example for the finding.
    pub documentation: &'static str,

    /// Evidence required to establish full rule coverage.
    pub prerequisites: Prerequisites,

    /// Validated parameter shape.
    pub settings: RuleSchema,

    /// Supported proposal guarantees.
    pub fixes: FixSupport,

    /// Exclusive evaluator ownership.
    pub producer: RuleProducer,
}

/// Named groups applied before caller overrides.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Default correctness-focused rules.
    Correctness,

    /// Correctness rules plus selected maintainability suggestions.
    Recommended,

    /// Every supported catalog rule.
    All,

    /// No enabled rules, before explicit overrides.
    None,
}

/// Returns supported rules in deterministic catalog order.
#[must_use]
pub fn rules() -> &'static [RuleDefinition] {
    &CATALOG
}
