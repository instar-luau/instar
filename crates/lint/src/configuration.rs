//! Instar rule configuration and validation.

use std::{collections::BTreeMap, num::NonZeroUsize};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Severity of an Instar lint rule.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Do not report findings.
    Allow,

    /// Report informational findings.
    Info,

    /// Report warnings.
    #[default]
    Warn,

    /// Report errors.
    Deny,
}

/// Settings for a rule enabled at warning severity by default.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Rule {
    /// Severity of findings.
    pub level: Level,
}

/// Settings for a rule allowed by default.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct AllowedRule {
    /// Severity of findings.
    pub level: Level,
}

impl Default for AllowedRule {
    fn default() -> Self {
        Self {
            level: Level::Allow,
        }
    }
}

macro_rules! configuration {
    (
        warn { $($warning:ident => $warning_identity:ident: $warning_documentation:literal,)* }
        allow { $($allowed:ident => $allowed_identity:ident: $allowed_documentation:literal,)* }
    ) => {
        /// Effective Instar linter settings after configuration inheritance.
        #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct Configuration {
            /// Included file patterns, inheriting global selection when omitted.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub include: Option<Vec<String>>,
            /// Excluded file patterns, inheriting global selection when omitted.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub exclude: Option<Vec<String>>,
            $(
                #[doc = $warning_documentation]
                pub $warning: Rule,
            )*
            $(
                #[doc = $allowed_documentation]
                pub $allowed: AllowedRule,
            )*
            /// Unused variable and optional parameter and loop binding checks.
            pub unused_variable: UnusedVariable,
            /// Cyclomatic complexity limit.
            pub high_cyclomatic_complexity: HighCyclomaticComplexity,
            /// Preference for immutable bindings.
            pub prefer_const: PreferConst,
            /// Project-specific deprecation mappings.
            pub deprecated: Deprecated,
            /// Restricted global name mappings.
            pub restricted_globals: RestrictedGlobals,
            /// Restricted module path mappings.
            pub restricted_module_paths: RestrictedModulePaths,
        }

        impl Default for Configuration {
            fn default() -> Self {
                Self {
                    include: None,
                    exclude: None,
                    $($warning: Rule::default(),)*
                    $($allowed: AllowedRule::default(),)*
                    unused_variable: UnusedVariable::default(),
                    high_cyclomatic_complexity: HighCyclomaticComplexity::default(),
                    prefer_const: PreferConst::default(),
                    deprecated: Deprecated::default(),
                    restricted_globals: RestrictedGlobals::default(),
                    restricted_module_paths: RestrictedModulePaths::default(),
                }
            }
        }

        /// Typed identities for configured Instar lint rules.
        pub mod identity {
            /// An Instar rule identity; names are only configuration and display boundaries.
            #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
            pub enum Rule {
                $(#[doc = $warning_documentation] $warning_identity,)*
                $(#[doc = $allowed_documentation] $allowed_identity,)*
                /// Unused local, parameter or loop binding.
                UnusedVariable,
                /// Excessive function cyclomatic complexity.
                HighCyclomaticComplexity,
                /// Immutable binding preference.
                PreferConst,
                /// A configured deprecated symbol.
                Deprecated,
                /// A configured restricted global.
                RestrictedGlobals,
                /// A configured restricted require request.
                RestrictedModulePaths,
            }

            impl Rule {
                /// Returns the unchanged configuration and presentation name.
                #[must_use]
                pub const fn name(self) -> &'static str {
                    match self {
                        $(Self::$warning_identity => stringify!($warning),)*
                        $(Self::$allowed_identity => stringify!($allowed),)*
                        Self::UnusedVariable => "unused_variable",
                        Self::HighCyclomaticComplexity => "high_cyclomatic_complexity",
                        Self::PreferConst => "prefer_const",
                        Self::Deprecated => "deprecated",
                        Self::RestrictedGlobals => "restricted_globals",
                        Self::RestrictedModulePaths => "restricted_module_paths",
                    }
                }
            }

            impl std::fmt::Display for Rule {
                fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    formatter.write_str(self.name())
                }
            }
        }

        impl Configuration {
            /// Returns every configured rule and its effective severity.
            #[must_use]
            pub fn rules(&self) -> Vec<(identity::Rule, Level)> {
                vec![
                    $((identity::Rule::$warning_identity, self.$warning.level),)*
                    $((identity::Rule::$allowed_identity, self.$allowed.level),)*
                    (identity::Rule::UnusedVariable, self.unused_variable.level),
                    (identity::Rule::HighCyclomaticComplexity, self.high_cyclomatic_complexity.level),
                    (identity::Rule::PreferConst, self.prefer_const.level),
                    (identity::Rule::Deprecated, self.deprecated.level),
                    (identity::Rule::RestrictedGlobals, self.restricted_globals.level),
                    (identity::Rule::RestrictedModulePaths, self.restricted_module_paths.level),
                ]
            }

            /// Returns the severity for an inventoried rule.
            #[must_use]
            pub const fn level(&self, rule: identity::Rule) -> Level {
                match rule {
                    $(identity::Rule::$warning_identity => self.$warning.level,)*
                    $(identity::Rule::$allowed_identity => self.$allowed.level,)*
                    identity::Rule::UnusedVariable => self.unused_variable.level,
                    identity::Rule::HighCyclomaticComplexity => self.high_cyclomatic_complexity.level,
                    identity::Rule::PreferConst => self.prefer_const.level,
                    identity::Rule::Deprecated => self.deprecated.level,
                    identity::Rule::RestrictedGlobals => self.restricted_globals.level,
                    identity::Rule::RestrictedModulePaths => self.restricted_module_paths.level,
                }
            }

            /// Whether inferred native binding types are needed.
            #[must_use]
            pub fn semantic(&self) -> bool {
                self.implicit_any_local.level != Level::Allow
                    || self.implicit_any_parameter.level != Level::Allow
            }
        }
    };
}

configuration! {
    warn {
        almost_swapped => AlmostSwapped: "Assignments that appear to swap values incorrectly.",
        bad_string_escape => BadStringEscape: "Invalid or suspicious string escapes.",
        compare_nan => CompareNan: "Comparisons against NaN.",
        constant_condition => ConstantCondition: "Conditions with constant outcomes.",
        constant_table_comparison => ConstantTableComparison: "Constant comparisons of table values.",
        length_as_condition => LengthAsCondition: "Lengths used directly as conditions.",
        mismatched_argument_count => MismatchedArgumentCount: "Calls with mismatched argument counts.",
        must_use => MustUse: "Discarded results that must be used.",
        zero_step_loop => ZeroStepLoop: "Numeric loops with zero steps.",
        divide_by_zero => DivideByZero: "Division by zero.",
        self_assignment => SelfAssignment: "Assignments of a binding to itself.",
        empty_if => EmptyIf: "Empty conditional branches.",
        empty_loop => EmptyLoop: "Empty loop bodies.",
        if_same_then_else => IfSameThenElse: "Identical conditional branches.",
        ignored_pcall_result => IgnoredPcallResult: "Ignored protected-call results.",
        mixed_table => MixedTable: "Mixed array and dictionary table entries.",
        unscoped_variables => UnscopedVariables: "Variables declared without explicit scope.",
        roblox_incorrect_color3_new_bounds => RobloxIncorrectColor3NewBounds: "Out-of-range Color3.new arguments.",
        roblox_manual_fromscale_or_fromoffset => RobloxManualFromscaleOrFromoffset: "Manual equivalents of fromScale or fromOffset.",
        roblox_prefer_get_players => RobloxPreferGetPlayers: "Player enumeration better expressed with `GetPlayers`.",
        roblox_suspicious_udim2_new => RobloxSuspiciousUdim2New: "Suspicious UDim2.new arguments.",
    }
    allow {
        implicit_any_local => ImplicitAnyLocal: "Local bindings with implicit any types.",
        implicit_any_parameter => ImplicitAnyParameter: "Parameters with implicit any types.",
        and_or_conditional => AndOrConditional: "Conditional expressions written with and/or.",
        collapsible_if => CollapsibleIf: "Nested conditionals that can be collapsed.",
        else_after_return => ElseAfterReturn: "Else branches following returning branches.",
        if_expression_assignment => IfExpressionAssignment: "Assignments expressible with if expressions.",
        negated_condition => NegatedCondition: "Conditionals with negated conditions.",
        non_const_require => NonConstRequire: "Require calls with nonconstant arguments.",
        parenthesized_conditions => ParenthesizedConditions: "Unnecessary parentheses around conditions.",
        global_usage => GlobalUsage: "Uses of global bindings.",
        type_check_inside_call => TypeCheckInsideCall: "Type checks inside call expressions.",
        loop_invariant_call => LoopInvariantCall: "Loop-invariant calls inside loops.",
        manual_table_clone => ManualTableClone: "Manual table cloning.",
        string_concat_in_loop => StringConcatInLoop: "Repeated string concatenation inside loops.",
    }
}

impl Configuration {
    /// Validates file-selection globs and ignored-name regular expressions.
    ///
    /// # Errors
    /// Returns an error for invalid include or exclude globs or an invalid
    /// unused-variable ignored-name regular expression.
    pub fn validate(&self) -> Result<(), String> {
        for (field, patterns) in [("include", &self.include), ("exclude", &self.exclude)] {
            if let Some(patterns) = patterns {
                for pattern in patterns {
                    glob::Pattern::new(pattern).map_err(|error| {
                        format!("lint.{field}: invalid glob {pattern:?}: {error}")
                    })?;
                }
            }
        }

        regex::Regex::new(&self.unused_variable.ignore_pattern).map_err(|error| {
            format!("lint.unused_variable.ignore_pattern: invalid regular expression: {error}")
        })?;

        Ok(())
    }
}

/// Unused binding rule settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnusedVariable {
    /// Severity of findings.
    pub level: Level,

    /// Include unused parameters.
    pub parameters: bool,

    /// Include unused loop variables.
    pub loop_variables: bool,

    /// Regular expression matching names exempt from the rule.
    pub ignore_pattern: String,
}

impl Default for UnusedVariable {
    fn default() -> Self {
        Self {
            level: Level::Warn,
            parameters: false,
            loop_variables: false,
            ignore_pattern: "^_".into(),
        }
    }
}

/// Cyclomatic complexity rule settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HighCyclomaticComplexity {
    /// Severity of findings.
    pub level: Level,

    /// Largest permitted cyclomatic complexity.
    pub maximum_complexity: NonZeroUsize,
}

impl Default for HighCyclomaticComplexity {
    fn default() -> Self {
        Self {
            level: Level::Allow,
            maximum_complexity: NonZeroUsize::new(40).expect("40 is nonzero"),
        }
    }
}

/// Immutable binding preference settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PreferConst {
    /// Severity of findings.
    pub level: Level,

    /// Keep locally mutated tables as local bindings.
    pub mutated_tables_stay_local: bool,
}

impl Default for PreferConst {
    fn default() -> Self {
        Self {
            level: Level::Allow,
            mutated_tables_stay_local: false,
        }
    }
}

/// Project-specific deprecation rule settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Deprecated {
    /// Severity of findings.
    pub level: Level,

    /// Deprecated symbol paths mapped to suggested replacements.
    pub paths: BTreeMap<String, String>,

    /// Report ambiguous method matches.
    pub ambiguous_methods: bool,
}

/// Restricted global rule settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedGlobals {
    /// Severity of findings.
    pub level: Level,

    /// Restricted global names mapped to reasons.
    pub names: BTreeMap<String, String>,
}

/// Restricted module path rule settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedModulePaths {
    /// Severity of findings.
    pub level: Level,

    /// Restricted module paths mapped to reasons.
    pub paths: BTreeMap<String, String>,
}
