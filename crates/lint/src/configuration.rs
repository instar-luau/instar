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
        warn { $($warning:ident => $warning_documentation:literal,)* }
        allow { $($allowed:ident => $allowed_documentation:literal,)* }
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
    };
}

configuration! {
    warn {
        almost_swapped => "Assignments that appear to swap values incorrectly.",
        bad_string_escape => "Invalid or suspicious string escapes.",
        compare_nan => "Comparisons against NaN.",
        constant_condition => "Conditions with constant outcomes.",
        constant_table_comparison => "Constant comparisons of table values.",
        length_as_condition => "Lengths used directly as conditions.",
        mismatched_argument_count => "Calls with mismatched argument counts.",
        must_use => "Discarded results that must be used.",
        zero_step_loop => "Numeric loops with zero steps.",
        divide_by_zero => "Division by zero.",
        self_assignment => "Assignments of a binding to itself.",
        empty_if => "Empty conditional branches.",
        empty_loop => "Empty loop bodies.",
        if_same_then_else => "Identical conditional branches.",
        ignored_pcall_result => "Ignored protected-call results.",
        mixed_table => "Mixed array and dictionary table entries.",
        unscoped_variables => "Variables declared without explicit scope.",
        roblox_incorrect_color3_new_bounds => "Out-of-range Color3.new arguments.",
        roblox_manual_fromscale_or_fromoffset => "Manual equivalents of fromScale or fromOffset.",
        roblox_prefer_get_players => "Player enumeration better expressed with `GetPlayers`.",
        roblox_suspicious_udim2_new => "Suspicious UDim2.new arguments.",
    }
    allow {
        implicit_any_local => "Local bindings with implicit any types.",
        implicit_any_parameter => "Parameters with implicit any types.",
        and_or_conditional => "Conditional expressions written with and/or.",
        collapsible_if => "Nested conditionals that can be collapsed.",
        else_after_return => "Else branches following returning branches.",
        if_expression_assignment => "Assignments expressible with if expressions.",
        negated_condition => "Conditionals with negated conditions.",
        non_const_require => "Require calls with nonconstant arguments.",
        parenthesized_conditions => "Unnecessary parentheses around conditions.",
        global_usage => "Uses of global bindings.",
        type_check_inside_call => "Type checks inside call expressions.",
        loop_invariant_call => "Loop-invariant calls inside loops.",
        manual_table_clone => "Manual table cloning.",
        string_concat_in_loop => "Repeated string concatenation inside loops.",
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
