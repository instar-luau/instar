use std::{fmt, sync::LazyLock};

use instar_luau::{SemanticCategory, UpstreamWarningIdentity};
use instar_shared::Options;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    RuleInput, RuleResult, RuleSettings,
    rules::{assignments, bindings, calls, control, expressions, roblox},
};

macro_rules! inventory {
    ($consumer:ident) => {
        $consumer! {
            AlmostSwapped, "almost_swapped", BasicSettings, Basic,
                "Assignments that overwrite a value before swapping it.",
                Assignments, LEXICAL, None, Instar(assignments::evaluate);
            BadStringEscape, "bad_string_escape", BasicSettings, Basic,
                "Invalid or suspicious string escapes.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            CompareNan, "compare_nan", BasicSettings, Basic,
                "Comparisons against NaN have fixed outcomes.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            ConstantCondition, "constant_condition", BasicSettings, Basic,
                "Conditions with statically known truth values.",
                Control, SYNTAX, None, Instar(control::evaluate);
            ConstantTableComparison, "constant_table_comparison", BasicSettings, Basic,
                "Fresh table values compared by identity.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            LengthAsCondition, "length_as_condition", BasicSettings, Basic,
                "Lengths used as booleans even though zero is truthy.",
                Control, SYNTAX, None, Instar(control::evaluate);
            MismatchedArgumentCount, "mismatched_argument_count", BasicSettings, Basic,
                "Calls with incompatible known lexical or imported argument constraints.",
                Calls, CALLS, None, Instar(calls::evaluate);
            MustUse, "must_use", BasicSettings, Basic,
                "Discarded results from known result-producing operations.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            ZeroStepLoop, "zero_step_loop", BasicSettings, Basic,
                "Numeric loops with a statically zero step.",
                Control, SYNTAX, None, Instar(control::evaluate);
            DivideByZero, "divide_by_zero", BasicSettings, Basic,
                "Statically identifiable division or modulo by zero.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            SelfAssignment, "self_assignment", BasicSettings, Basic,
                "Assignments of a binding to itself.",
                Assignments, LEXICAL, Bindings, Instar(assignments::evaluate);
            EmptyIf, "empty_if", BasicSettings, Basic,
                "Conditional branches with empty bodies.",
                Control, SYNTAX, None, Instar(control::evaluate);
            EmptyLoop, "empty_loop", BasicSettings, Basic,
                "Empty loops that cannot make progress.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IfSameThenElse, "if_same_then_else", BasicSettings, Basic,
                "Conditional branches with identical bodies.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IgnoredPcallResult, "ignored_pcall_result", BasicSettings, Basic,
                "Protected-call success results that are ignored.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            MixedTable, "mixed_table", BasicSettings, Basic,
                "Table literals mixing positional and keyed entries.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            UnscopedVariables, "unscoped_variables", BasicSettings, Basic,
                "Assignments introducing undeclared global bindings.",
                Bindings, ENVIRONMENT, None, Instar(bindings::evaluate);
            RobloxIncorrectColor3NewBounds, "roblox_incorrect_color3_new_bounds", BasicSettings, Basic,
                "`Color3.new` components outside zero through one.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxManualFromscaleOrFromoffset, "roblox_manual_fromscale_or_fromoffset", BasicSettings, Basic,
                "`UDim2.new` calls expressible as `fromScale` or `fromOffset`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxPreferGetPlayers, "roblox_prefer_get_players", BasicSettings, Basic,
                "Player enumeration better expressed with `GetPlayers`.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            RobloxSuspiciousUdim2New, "roblox_suspicious_udim2_new", BasicSettings, Basic,
                "`UDim2.new` calls with suspicious scale and offset components.",
                Roblox, ENVIRONMENT, None, Instar(roblox::evaluate);
            ImplicitAnyLocal, "implicit_any_local", BasicSettings, Basic,
                "Unannotated local bindings with known inferred any types.",
                Bindings, TYPES, None, Instar(bindings::evaluate);
            ImplicitAnyParameter, "implicit_any_parameter", BasicSettings, Basic,
                "Unannotated parameters with known inferred any types.",
                Bindings, TYPES, None, Instar(bindings::evaluate);
            AndOrConditional, "and_or_conditional", BasicSettings, Basic,
                "Conditional and/or expressions better expressed with if.",
                Expressions, SYNTAX, None, Instar(expressions::evaluate);
            CollapsibleIf, "collapsible_if", BasicSettings, Basic,
                "Nested single-branch conditionals that can be combined.",
                Control, SYNTAX, None, Instar(control::evaluate);
            ElseAfterReturn, "else_after_return", BasicSettings, Basic,
                "Else branches following branches that always return.",
                Control, SYNTAX, None, Instar(control::evaluate);
            IfExpressionAssignment, "if_expression_assignment", BasicSettings, Basic,
                "Matching assignments expressible as conditional expressions.",
                Control, LEXICAL, None, Instar(control::evaluate);
            NegatedCondition, "negated_condition", BasicSettings, Basic,
                "Negated conditions expressible by reversing branches.",
                Control, SYNTAX, None, Instar(control::evaluate);
            NonConstRequire, "non_const_require", BasicSettings, Basic,
                "Require calls with nonconstant arguments.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            ParenthesizedConditions, "parenthesized_conditions", BasicSettings, Basic,
                "Unnecessary parentheses around conditions.",
                Control, SYNTAX, Syntax, Instar(control::evaluate);
            GlobalUsage, "global_usage", BasicSettings, Basic,
                "References to global bindings.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            TypeCheckInsideCall, "type_check_inside_call", BasicSettings, Basic,
                "Comparisons misplaced inside type or typeof calls.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            LoopInvariantCall, "loop_invariant_call", BasicSettings, Basic,
                "Calls proven safe and independent of loop effects.",
                Calls, EFFECTS, None, Instar(calls::evaluate);
            ManualTableClone, "manual_table_clone", BasicSettings, Basic,
                "Table-copy loops expressible with table.clone.",
                Expressions, LEXICAL, None, Instar(expressions::evaluate);
            StringConcatInLoop, "string_concat_in_loop", BasicSettings, Basic,
                "Repeated string concatenation inside loops.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            UnusedVariable, "unused_variable", UnusedBindings, UnusedBindings,
                "Unread local bindings, parameters, and loop variables.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            PreferConst, "prefer_const", ImmutableBindings, ImmutableBindings,
                "Initialized local bindings that are never reassigned.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            Deprecated, "deprecated", DeprecatedSymbols, DeprecatedSymbols,
                "Project-deprecated symbol paths with replacements.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            RestrictedGlobals, "restricted_globals", RestrictedGlobals, RestrictedGlobals,
                "Uses of project-restricted global names.",
                Bindings, LEXICAL, None, Instar(bindings::evaluate);
            RestrictedModulePaths, "restricted_module_paths", RestrictedImports, RestrictedImports,
                "Requires matching project-restricted module paths.",
                Calls, LEXICAL, None, Instar(calls::evaluate);
            UnknownGlobal, "UnknownGlobal", BasicSettings, Basic,
                "Unknown global references.",
                Bindings, ENVIRONMENT, None, Upstream;
            DeprecatedGlobal, "DeprecatedGlobal", BasicSettings, Basic,
                "Deprecated global references.",
                Bindings, ENVIRONMENT, None, Upstream;
            GlobalUsedAsLocal, "GlobalUsedAsLocal", BasicSettings, Basic,
                "Global bindings better expressed as locals.",
                Bindings, ENVIRONMENT, None, Upstream;
            LocalShadow, "LocalShadow", BasicSettings, Basic,
                "Local declarations shadowing existing bindings.",
                Bindings, ENVIRONMENT, None, Upstream;
            SameLineStatement, "SameLineStatement", BasicSettings, Basic,
                "Multiple statements on one line.",
                Control, SYNTAX, None, Upstream;
            MultiLineStatement, "MultiLineStatement", BasicSettings, Basic,
                "Misleading multiline statement layout.",
                Control, SYNTAX, None, Upstream;
            LocalUnused, "LocalUnused", BasicSettings, Basic,
                "Unused local bindings.",
                Bindings, LEXICAL, None, Upstream;
            FunctionUnused, "FunctionUnused", BasicSettings, Basic,
                "Unused local functions.",
                Bindings, LEXICAL, None, Upstream;
            ImportUnused, "ImportUnused", BasicSettings, Basic,
                "Unused imports.",
                Bindings, LEXICAL, None, Upstream;
            BuiltinGlobalWrite, "BuiltinGlobalWrite", BasicSettings, Basic,
                "Writes to builtin globals.",
                Bindings, ENVIRONMENT, None, Upstream;
            PlaceholderRead, "PlaceholderRead", BasicSettings, Basic,
                "Reads from placeholder bindings.",
                Bindings, ENVIRONMENT, None, Upstream;
            UnreachableCode, "UnreachableCode", BasicSettings, Basic,
                "Unreachable code.",
                Control, SYNTAX, None, Upstream;
            UnknownType, "UnknownType", BasicSettings, Basic,
                "Unsupported names in runtime type tests.",
                Calls, SYNTAX, None, Upstream;
            ForRange, "ForRange", BasicSettings, Basic,
                "Suspicious numeric loop ranges.",
                Control, SYNTAX, None, Upstream;
            UnbalancedAssignment, "UnbalancedAssignment", BasicSettings, Basic,
                "Assignment target and value mismatches.",
                Assignments, SYNTAX, None, Upstream;
            ImplicitReturn, "ImplicitReturn", BasicSettings, Basic,
                "Inconsistent explicit and implicit returns.",
                Control, SYNTAX, None, Upstream;
            DuplicateLocal, "DuplicateLocal", BasicSettings, Basic,
                "Duplicate local declarations.",
                Bindings, LEXICAL, None, Upstream;
            FormatString, "FormatString", BasicSettings, Basic,
                "Invalid format strings or arguments.",
                Calls, SYNTAX, None, Upstream;
            TableLiteral, "TableLiteral", BasicSettings, Basic,
                "Suspicious or duplicate table entries.",
                Expressions, INFERRED, None, Upstream;
            UninitializedLocal, "UninitializedLocal", BasicSettings, Basic,
                "Reads before initialization.",
                Bindings, LEXICAL, None, Upstream;
            DuplicateFunction, "DuplicateFunction", BasicSettings, Basic,
                "Duplicate function definitions.",
                Bindings, LEXICAL, None, Upstream;
            DeprecatedApi, "DeprecatedApi", BasicSettings, Basic,
                "Deprecated API references.",
                Calls, INFERRED, None, Upstream;
            TableOperations, "TableOperations", BasicSettings, Basic,
                "Suspicious table operations.",
                Calls, INFERRED, None, Upstream;
            DuplicateCondition, "DuplicateCondition", BasicSettings, Basic,
                "Repeated conditional tests.",
                Control, SYNTAX, None, Upstream;
            MisleadingAndOr, "MisleadingAndOr", BasicSettings, Basic,
                "Misleading conditional and/or results.",
                Expressions, SYNTAX, None, Upstream;
            CommentDirective, "CommentDirective", BasicSettings, Basic,
                "Invalid analysis directives.",
                Control, SYNTAX, None, Upstream;
            IntegerParsing, "IntegerParsing", BasicSettings, Basic,
                "Unexpected integer literal representation.",
                Expressions, SYNTAX, None, Upstream;
            ComparisonPrecedence, "ComparisonPrecedence", BasicSettings, Basic,
                "Misleading comparison precedence.",
                Expressions, SYNTAX, None, Upstream;
            RedundantNativeAttribute, "RedundantNativeAttribute", BasicSettings, Basic,
                "Redundant native compilation annotations.",
                Control, SYNTAX, None, Upstream;
        }
    };
}

macro_rules! define_catalog {
    ($(
        $identity:ident, $name:literal, $parameters:ty, $schema:ident,
        $summary:literal,
        $category:ident, $prerequisites:expr, $fixes:ident, $producer:ident $(($evaluator:path))?;
    )*) => {
        static CATALOG: LazyLock<Vec<RuleDefinition>> = LazyLock::new(|| vec![$(
            RuleDefinition {
                identity: RuleIdentity::$identity,
                category: RuleCategory::$category,
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
