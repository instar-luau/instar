//! Project configuration formats, Luau settings, and formatting options.

use std::{collections::BTreeMap, io, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vermis::tree::{NodeIndex, NodeKind, Tree};

use crate::{invalid, string_value};

/// Diagnostic severity.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LintLevel {
    /// Suppress the rule.
    Allow,

    /// Publish informational diagnostics.
    Info,

    /// Publish non-fatal warnings.
    #[default]
    Warn,

    /// Publish fatal diagnostics.
    Deny,
}

/// Rule settings without additional detection options.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RuleConfig {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,
}

macro_rules! lint_catalog {
    (
        native { $($native:ident => $warning:literal),+ $(,)? }
        custom { $($rule:ident : $configuration:ident => $default:ident),+ $(,)? }
    ) => {
        /// Lint configuration and file selection.
        #[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        pub struct LintConfig {
            /// Include globs; omission inherits, an explicit list replaces.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub include: Option<Vec<String>>,

            /// Exclude globs; omission inherits, an explicit list replaces.
            #[serde(skip_serializing_if = "Option::is_none")]
            #[schemars(with = "Vec<String>")]
            pub exclude: Option<Vec<String>>,

            $(#[doc = concat!("Settings for the native `", $warning, "` warning.")]
              #[schemars(extend("default" = lint_schema_default().$native))]
              pub $native: RuleConfig,)+

            $(#[doc = concat!("Settings for the `", stringify!($rule), "` rule.")]
              #[schemars(extend("default" = lint_schema_default().$rule))]
              pub $rule: $configuration,)+
        }

        impl LintConfig {
            /// Merges an inherited lint layer, preserving explicit overrides.
            pub fn merge(&mut self, layer: &Self) {
                if layer.include.is_some() {
                    self.include.clone_from(&layer.include);
                }
                if layer.exclude.is_some() {
                    self.exclude.clone_from(&layer.exclude);
                }
                $(self.$native.level = layer.$native.level.or(self.$native.level);)+
                $(self.$rule.level = layer.$rule.level.or(self.$rule.level);)+
                self.merge_options(layer);
            }

            /// Resolves the effective level for a `snake_case` lint rule name.
            #[must_use]
            pub fn level(&self, rule: &str) -> LintLevel {
                match rule {
                    $(stringify!($native) => self.$native.level.unwrap_or(LintLevel::Warn),)+
                    $(stringify!($rule) => self.$rule.level.unwrap_or(LintLevel::$default),)+
                    _ => LintLevel::Allow,
                }
            }

            /// Returns an explicit override for a native `PascalCase` warning name.
            #[must_use]
            pub fn native_level(&self, warning: &str) -> Option<LintLevel> {
                match warning {
                    $($warning => self.$native.level,)+
                    _ => None,
                }
            }

            /// Returns explicitly configured native warning levels.
            pub fn native_overrides(&self) -> impl Iterator<Item = (&'static str, LintLevel)> {
                [$(($warning, self.$native.level),)+]
                    .into_iter()
                    .filter_map(|(warning, level)| level.map(|level| (warning, level)))
            }
        }

        fn lint_schema_default() -> LintConfig {
            let mut config = LintConfig::default();
            $(config.$native.level = Some(LintLevel::Warn);)+
            $(config.$rule.level = Some(LintLevel::$default);)+
            config.unused_variable.parameters = Some(false);
            config.unused_variable.loop_variables = Some(false);
            config.unused_variable.ignore_pattern = Some("^_".to_owned());
            config.high_cyclomatic_complexity.maximum_complexity = Some(40);
            config.prefer_const.mutated_tables_stay_local = Some(false);
            config.deprecated.ambiguous_methods = Some(false);
            config
        }
    };
}

lint_catalog! {
    native {
        unknown_global => "UnknownGlobal",
        deprecated_global => "DeprecatedGlobal",
        global_used_as_local => "GlobalUsedAsLocal",
        local_shadow => "LocalShadow",
        same_line_statement => "SameLineStatement",
        multi_line_statement => "MultiLineStatement",
        local_unused => "LocalUnused",
        function_unused => "FunctionUnused",
        import_unused => "ImportUnused",
        builtin_global_write => "BuiltinGlobalWrite",
        placeholder_read => "PlaceholderRead",
        unreachable_code => "UnreachableCode",
        unknown_type => "UnknownType",
        for_range => "ForRange",
        unbalanced_assignment => "UnbalancedAssignment",
        implicit_return => "ImplicitReturn",
        duplicate_local => "DuplicateLocal",
        format_string => "FormatString",
        table_literal => "TableLiteral",
        uninitialized_local => "UninitializedLocal",
        duplicate_function => "DuplicateFunction",
        deprecated_api => "DeprecatedApi",
        table_operations => "TableOperations",
        duplicate_condition => "DuplicateCondition",
        misleading_and_or => "MisleadingAndOr",
        comment_directive => "CommentDirective",
        integer_parsing => "IntegerParsing",
        comparison_precedence => "ComparisonPrecedence",
        redundant_native_attribute => "RedundantNativeAttribute",
    }
    custom {
        almost_swapped: RuleConfig => Warn,
        bad_string_escape: RuleConfig => Warn,
        compare_nan: RuleConfig => Warn,
        constant_condition: RuleConfig => Warn,
        constant_table_comparison: RuleConfig => Warn,
        length_as_condition: RuleConfig => Deny,
        mismatched_arg_count: RuleConfig => Warn,
        must_use: RuleConfig => Warn,
        unused_variable: UnusedVariableOptions => Warn,
        type_check_inside_call: RuleConfig => Warn,
        zero_step_loop: RuleConfig => Deny,
        divide_by_zero: RuleConfig => Warn,
        empty_if: RuleConfig => Warn,
        empty_loop: RuleConfig => Warn,
        global_usage: RuleConfig => Warn,
        if_same_then_else: RuleConfig => Warn,
        ignored_pcall_result: RuleConfig => Warn,
        implicit_any_local: RuleConfig => Warn,
        implicit_any_parameter: RuleConfig => Allow,
        mixed_table: RuleConfig => Warn,
        self_assignment: RuleConfig => Warn,
        unscoped_variables: RuleConfig => Warn,
        and_or_conditional: RuleConfig => Allow,
        collapsible_if: RuleConfig => Allow,
        deprecated: DeprecatedOptions => Warn,
        else_after_return: RuleConfig => Allow,
        if_expression_assignment: RuleConfig => Allow,
        negated_condition: RuleConfig => Allow,
        non_const_require: RuleConfig => Allow,
        parenthese_conditions: RuleConfig => Warn,
        prefer_const: PreferConstOptions => Allow,
        restricted_globals: RestrictedGlobalsOptions => Warn,
        restricted_module_paths: RestrictedModulePathsOptions => Warn,
        high_cyclomatic_complexity: HighCyclomaticComplexityOptions => Allow,
        loop_invariant_call: RuleConfig => Warn,
        manual_table_clone: RuleConfig => Warn,
        string_concat_in_loop: RuleConfig => Warn,
        roblox_incorrect_color3_new_bounds: RuleConfig => Warn,
        roblox_manual_fromscale_or_fromoffset: RuleConfig => Warn,
        roblox_prefer_get_players: RuleConfig => Warn,
        roblox_suspicious_udim2_new: RuleConfig => Warn,
    }
}

/// Optional unused-binding detection settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnusedVariableOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Check otherwise unused unannotated parameters.
    #[schemars(with = "bool", extend("default" = false))]
    pub parameters: Option<bool>,

    /// Check otherwise unused loop variables.
    #[schemars(with = "bool", extend("default" = false))]
    pub loop_variables: Option<bool>,

    /// Pattern for ignored binding names.
    #[schemars(with = "String", extend("default" = "^_"))]
    pub ignore_pattern: Option<String>,
}

impl UnusedVariableOptions {
    /// Returns whether parameters are checked.
    #[must_use]
    pub fn parameters(&self) -> bool {
        self.parameters.unwrap_or(false)
    }

    /// Returns whether loop bindings are checked.
    #[must_use]
    pub fn loop_variables(&self) -> bool {
        self.loop_variables.unwrap_or(false)
    }

    /// Returns the ignored-name regular expression.
    #[must_use]
    pub fn ignore_pattern(&self) -> &str {
        self.ignore_pattern.as_deref().unwrap_or("^_")
    }
}

/// Maximum cyclomatic complexity setting.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct HighCyclomaticComplexityOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Report function scores strictly above this limit.
    #[schemars(with = "usize", extend("default" = 40))]
    pub maximum_complexity: Option<usize>,
}

impl HighCyclomaticComplexityOptions {
    /// Returns the maximum allowed branch score.
    #[must_use]
    pub fn maximum_complexity(&self) -> usize {
        self.maximum_complexity.unwrap_or(40)
    }
}

/// Constant-binding detection settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PreferConstOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Exempt locally mutated tables from prefer-const checks.
    #[schemars(with = "bool", extend("default" = false))]
    pub mutated_tables_stay_local: Option<bool>,
}

impl PreferConstOptions {
    /// Returns whether locally mutated tables remain local.
    #[must_use]
    pub fn mutated_tables_stay_local(&self) -> bool {
        self.mutated_tables_stay_local.unwrap_or(false)
    }
}

/// Project-specific deprecation settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct DeprecatedOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Deprecated API path to replacement mappings.
    /// Child keys override inherited keys; omission or an empty map preserves them.
    pub paths: BTreeMap<String, String>,

    /// Report calls whose method name cannot be resolved uniquely.
    #[schemars(with = "bool", extend("default" = false))]
    pub ambiguous_methods: Option<bool>,
}

impl DeprecatedOptions {
    /// Returns whether ambiguous method calls are checked.
    #[must_use]
    pub fn ambiguous_methods(&self) -> bool {
        self.ambiguous_methods.unwrap_or(false)
    }
}

/// Restricted global name settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedGlobalsOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Forbidden global names and reasons.
    pub names: BTreeMap<String, String>,
}

/// Restricted module path settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedModulePathsOptions {
    /// Diagnostic severity; omitted values inherit.
    #[schemars(with = "LintLevel")]
    pub level: Option<LintLevel>,

    /// Forbidden literal require paths and reasons.
    pub paths: BTreeMap<String, String>,
}

/// Manifest Luau settings; lint rules belong under `[lint]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ManifestLuauConfig {
    /// Typechecking mode.
    pub language_mode: Option<Mode>,

    /// Whether type errors are reported.
    pub type_errors: Option<bool>,

    /// Additional Luau globals.
    pub globals: Option<Vec<String>>,

    /// Luau module aliases.
    pub aliases: BTreeMap<String, String>,

    /// Definition file paths.
    pub definitions: BTreeMap<String, String>,

    /// Documentation file paths and URLs.
    pub documentation: Vec<String>,

    /// Explicit process-global bool or integer Luau flag values applied after synced values.
    /// Child keys override inherited keys; values are shared across projects in one process.
    pub flags: BTreeMap<String, FlagValue>,
}

impl From<ManifestLuauConfig> for LuauConfig {
    fn from(config: ManifestLuauConfig) -> Self {
        Self {
            language_mode: config.language_mode,
            lint: BTreeMap::new(),
            lint_errors: None,
            type_errors: config.type_errors,
            globals: config.globals,
            aliases: config.aliases,
            definitions: config.definitions,
            documentation: config.documentation,
            flags: config.flags,
        }
    }
}

impl LintConfig {
    fn merge_options(&mut self, layer: &Self) {
        self.unused_variable.parameters = layer
            .unused_variable
            .parameters
            .or(self.unused_variable.parameters);

        self.unused_variable.loop_variables = layer
            .unused_variable
            .loop_variables
            .or(self.unused_variable.loop_variables);

        if layer.unused_variable.ignore_pattern.is_some() {
            self.unused_variable
                .ignore_pattern
                .clone_from(&layer.unused_variable.ignore_pattern);
        }

        self.high_cyclomatic_complexity.maximum_complexity = layer
            .high_cyclomatic_complexity
            .maximum_complexity
            .or(self.high_cyclomatic_complexity.maximum_complexity);

        self.prefer_const.mutated_tables_stay_local = layer
            .prefer_const
            .mutated_tables_stay_local
            .or(self.prefer_const.mutated_tables_stay_local);

        self.deprecated.paths.extend(layer.deprecated.paths.clone());

        self.deprecated.ambiguous_methods = layer
            .deprecated
            .ambiguous_methods
            .or(self.deprecated.ambiguous_methods);

        self.restricted_globals
            .names
            .extend(layer.restricted_globals.names.clone());

        self.restricted_module_paths
            .paths
            .extend(layer.restricted_module_paths.paths.clone());
    }

    /// Validates rule option patterns.
    ///
    /// # Errors
    /// Returns an error for an invalid ignored-name regular expression.
    pub fn validate(&self) -> io::Result<()> {
        if let Some(pattern) = &self.unused_variable.ignore_pattern {
            regex::Regex::new(pattern).map_err(|error| {
                invalid(format!("invalid unused_variable.ignore_pattern: {error}"))
            })?;
        }

        Ok(())
    }
}

/// Project manifest. Luau settings live in `[luau]`; formatting settings in `[format]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Global include globs relative to this manifest. Omission inherits;
    /// an explicit list replaces, and `[]` allows every path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Global exclude globs. Omission inherits; an explicit list replaces,
    /// and `[]` clears exclusions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,

    /// File selection for syntax and type checking, falling back to global fields.
    pub check: FileFilter,

    /// Formatting style and file selection, falling back to global fields.
    #[schemars(extend("default" = format_schema_default()))]
    pub format: FormatConfig,

    /// Lint rules, options, and file selection, falling back to global fields.
    #[schemars(extend("default" = lint_schema_default()))]
    pub lint: LintConfig,

    /// Workspace indexing and independent autoimport settings.
    pub lsp: LspConfig,

    /// Luau typechecking, resolution, and process-global fast-flag settings.
    pub luau: ManifestLuauConfig,

    /// Optional Roblox sourcemap configuration.
    pub roblox: RobloxConfig,
}

/// Service-specific globs, falling back to global fields.
/// Omitted fields inherit; explicit lists replace, and `[]` clears a field.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FileFilter {
    /// Include globs relative to their manifest; an empty effective list allows every path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude globs relative to their manifest; exclusions win over effective includes.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
}

/// Workspace indexing and independent autoimport settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LspConfig {
    /// Workspace file selection, independent of autoimport selection.
    pub index: FileFilter,

    /// Autoimport preferences and selection, independent of indexing.
    pub imports: ImportsConfig,
}

/// Autoimport preferences and file selection, falling back to global fields.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImportsConfig {
    /// Preferred require path style; omission inherits or defaults to instance paths.
    pub require: Option<RequireStyle>,

    /// Generated binding style; omission inherits or defaults to local bindings.
    pub binding: Option<BindingStyle>,

    /// Include globs relative to their manifest; omission inherits, an explicit list replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude globs relative to their manifest; omission inherits, an explicit list replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
}

impl ImportsConfig {
    pub(super) fn merge(&mut self, layer: &Self) {
        self.require = layer.require.or(self.require);
        self.binding = layer.binding.or(self.binding);

        if layer.include.is_some() {
            self.include.clone_from(&layer.include);
        }

        if layer.exclude.is_some() {
            self.exclude.clone_from(&layer.exclude);
        }
    }
}

/// Preferred path style for generated require calls.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RequireStyle {
    /// Prefer instance paths when available.
    #[default]
    Instance,

    /// Prefer string paths.
    String,
}

/// Binding style for generated autoimports.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BindingStyle {
    /// Generate local bindings.
    #[default]
    Local,

    /// Generate constant bindings.
    Const,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Optional formatting overrides and file selection. Omitted values inherit;
/// schema defaults describe the root configuration.
pub struct FormatConfig {
    /// Include globs for formatting; omission inherits, an explicit list replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude globs for formatting; omission inherits, an explicit list replaces.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,

    /// Target line width.
    #[schemars(with = "usize", extend("default" = FormatOptions::default().width))]
    pub width: Option<usize>,

    /// Indentation character.
    #[schemars(with = "IndentStyle", extend("default" = FormatOptions::default().indent_style))]
    pub indent_style: Option<IndentStyle>,

    /// Spaces per level or display width of a tab.
    #[schemars(with = "usize", extend("default" = FormatOptions::default().indent_width))]
    pub indent_width: Option<usize>,

    /// Output line ending.
    #[schemars(with = "LineEnding", extend("default" = FormatOptions::default().line_ending))]
    pub line_ending: Option<LineEnding>,

    /// Whether output ends in a newline.
    #[schemars(with = "bool", extend("default" = FormatOptions::default().final_newline))]
    pub final_newline: Option<bool>,

    /// String delimiter policy.
    #[schemars(with = "QuoteStyle", extend("default" = FormatOptions::default().quote_style))]
    pub quote_style: Option<QuoteStyle>,

    /// Decimal leading-zero policy.
    #[schemars(with = "LeadingZero", extend("default" = FormatOptions::default().leading_zero))]
    pub leading_zero: Option<LeadingZero>,

    /// Statement separator policy.
    #[schemars(with = "Semicolons", extend("default" = FormatOptions::default().semicolons))]
    pub semicolons: Option<Semicolons>,

    /// Whitespace settings.
    #[schemars(with = "PartialSpacingOptions", extend("default" = FormatOptions::default().spacing))]
    pub spacing: Option<PartialSpacingOptions>,

    /// Call formatting settings.
    #[schemars(with = "PartialCallsOptions", extend("default" = FormatOptions::default().calls))]
    pub calls: Option<PartialCallsOptions>,

    /// Function parameter formatting settings.
    #[schemars(with = "PartialParametersOptions", extend("default" = FormatOptions::default().parameters))]
    pub parameters: Option<PartialParametersOptions>,

    /// Value table formatting settings.
    #[schemars(with = "PartialTablesOptions", extend("default" = FormatOptions::default().tables))]
    pub tables: Option<PartialTablesOptions>,

    /// Block formatting settings.
    #[schemars(with = "PartialBlocksOptions", extend("default" = FormatOptions::default().blocks))]
    pub blocks: Option<PartialBlocksOptions>,

    /// Named call chain formatting settings.
    #[schemars(with = "PartialChainsOptions", extend("default" = FormatOptions::default().chains))]
    pub chains: Option<PartialChainsOptions>,

    /// If-expression formatting settings.
    #[schemars(with = "PartialIfExpressionsOptions", extend("default" = FormatOptions::default().if_expressions))]
    pub if_expressions: Option<PartialIfExpressionsOptions>,

    /// Type layout settings.
    #[schemars(with = "PartialTypesOptions", extend("default" = FormatOptions::default().types))]
    pub types: Option<PartialTypesOptions>,

    /// Require ordering settings.
    #[schemars(with = "PartialRequiresOptions", extend("default" = FormatOptions::default().requires))]
    pub requires: Option<PartialRequiresOptions>,
}

fn format_schema_default() -> Value {
    let mut config =
        serde_json::to_value(FormatConfig::default()).expect("default format config serializes");

    let Value::Object(style) =
        serde_json::to_value(FormatOptions::default()).expect("default format options serialize")
    else {
        unreachable!("format options serialize as an object");
    };

    config
        .as_object_mut()
        .expect("format config serializes as an object")
        .extend(style);

    config
}

macro_rules! partial {
    ($name:ident, $section:ident { $($field:ident : $ty:ty => $schema_ty:tt),+ $(,)? }) => {
        #[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        #[doc = concat!("Optional overrides for ", stringify!($name), ".")]
        pub struct $name {
            $(#[doc = concat!("Override for `", stringify!($field), "`.")]
              #[schemars(with = $schema_ty, extend("default" = FormatOptions::default().$section.$field))]
              pub $field: Option<$ty>),+
        }
    };
}

partial!(PartialSpacingOptions, spacing {
    braces: bool => "bool",
    interpolation: bool => "bool",
    parentheses: bool => "bool",
    brackets: bool => "bool",
    before_function_parentheses: BeforeFunctionParentheses => "BeforeFunctionParentheses"
});

partial!(PartialCallsOptions, calls {
    parentheses: CallParentheses => "CallParentheses",
    wrap: Wrap => "Wrap",
    layout: CallLayout => "CallLayout"
});

partial!(PartialParametersOptions, parameters { wrap: Wrap => "Wrap" });

partial!(PartialTablesOptions, tables {
    wrap: Wrap => "Wrap",
    trailing_comma: TrailingComma => "TrailingComma",
    blank_lines: TableBlankLines => "TableBlankLines"
});

partial!(PartialBlocksOptions, blocks {
    edge_blank_lines: EdgeBlankLines => "EdgeBlankLines",
    simple_bodies: SimpleBodies => "SimpleBodies"
});

partial!(PartialChainsOptions, chains {
    wrap: Wrap => "Wrap",
    layout: ChainLayout => "ChainLayout"
});

partial!(PartialIfExpressionsOptions, if_expressions {
    wrap: Wrap => "Wrap",
    layout: IfExpressionLayout => "IfExpressionLayout",
    placement: IfExpressionPlacement => "IfExpressionPlacement"
});

partial!(PartialTypesOptions, types {
    tables: PartialTypeTablesOptions => "PartialTypeTablesOptions",
    operators: PartialTypeOperatorsOptions => "PartialTypeOperatorsOptions"
});

/// Optional type table formatting overrides.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PartialTypeTablesOptions {
    /// Type table wrapping policy.
    #[schemars(with = "Wrap", extend("default" = FormatOptions::default().types.tables.wrap))]
    pub wrap: Option<Wrap>,

    /// Type table field separator.
    #[schemars(with = "TypeTableSeparator", extend("default" = FormatOptions::default().types.tables.separator))]
    pub separator: Option<TypeTableSeparator>,
}

/// Optional type operator formatting overrides.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct PartialTypeOperatorsOptions {
    /// Union and intersection wrapping policy.
    #[schemars(with = "Wrap", extend("default" = FormatOptions::default().types.operators.wrap))]
    pub wrap: Option<Wrap>,
}

partial!(PartialRequiresOptions, requires {
    order: RequireOrder => "RequireOrder",
    blank_lines: RequireBlankLines => "RequireBlankLines",
    groups: Vec<RequireGroup> => "Vec<RequireGroup>"
});

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Effective formatting settings after project inheritance.
pub struct FormatOptions {
    /// Target line width.
    pub width: usize,

    /// Indentation character.
    pub indent_style: IndentStyle,

    /// Spaces per level or display width of a tab.
    pub indent_width: usize,

    /// Output line ending.
    pub line_ending: LineEnding,

    /// Whether output ends in a newline.
    pub final_newline: bool,

    /// String delimiter policy.
    pub quote_style: QuoteStyle,

    /// Decimal leading-zero policy.
    pub leading_zero: LeadingZero,

    /// Statement separator policy.
    pub semicolons: Semicolons,

    /// Whitespace settings.
    pub spacing: SpacingOptions,

    /// Call formatting settings.
    pub calls: CallsOptions,

    /// Function parameter formatting settings.
    pub parameters: ParametersOptions,

    /// Value table formatting settings.
    pub tables: TablesOptions,

    /// Block formatting settings.
    pub blocks: BlocksOptions,

    /// Named call chain formatting settings.
    pub chains: ChainsOptions,

    /// If-expression formatting settings.
    pub if_expressions: IfExpressionsOptions,

    /// Type layout settings.
    pub types: TypesOptions,

    /// Require ordering settings.
    pub requires: RequiresOptions,
}

macro_rules! options {
    ($name:ident { $($field:ident : $ty:ty = $default:expr),+ $(,)? }) => {
        #[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
        #[serde(default, deny_unknown_fields)]
        #[doc = concat!("Effective settings for ", stringify!($name), ".")]
        pub struct $name {
            $(#[doc = concat!("`", stringify!($field), "` formatting setting.")]
              pub $field: $ty),+
        }
        impl Default for $name { fn default() -> Self { Self { $($field: $default),+ } } }
    };
}

options!(SpacingOptions {
    braces: bool = true,
    interpolation: bool = false,
    parentheses: bool = false,
    brackets: bool = false,
    before_function_parentheses: BeforeFunctionParentheses = BeforeFunctionParentheses::Never
});

options!(CallsOptions {
    parentheses: CallParentheses = CallParentheses::Always,
    wrap: Wrap = Wrap::Auto,
    layout: CallLayout = CallLayout::Vertical
});

options!(ParametersOptions {
    wrap: Wrap = Wrap::Auto
});

options!(TablesOptions {
    wrap: Wrap = Wrap::Preserve,
    trailing_comma: TrailingComma = TrailingComma::Multiline,
    blank_lines: TableBlankLines = TableBlankLines::Preserve
});

options!(BlocksOptions {
    edge_blank_lines: EdgeBlankLines = EdgeBlankLines::Remove,
    simple_bodies: SimpleBodies = SimpleBodies::Expand
});

options!(ChainsOptions {
    wrap: Wrap = Wrap::Auto,
    layout: ChainLayout = ChainLayout::Method
});

options!(IfExpressionsOptions {
    wrap: Wrap = Wrap::Auto,
    layout: IfExpressionLayout = IfExpressionLayout::Block,
    placement: IfExpressionPlacement = IfExpressionPlacement::SameLine
});

options!(TypesOptions {
    tables: TypeTablesOptions = TypeTablesOptions::default(),
    operators: TypeOperatorsOptions = TypeOperatorsOptions::default()
});

options!(TypeTablesOptions {
    wrap: Wrap = Wrap::Auto,
    separator: TypeTableSeparator = TypeTableSeparator::Comma
});

options!(TypeOperatorsOptions {
    wrap: Wrap = Wrap::Auto
});

options!(RequiresOptions { order: RequireOrder = RequireOrder::Grouped, blank_lines: RequireBlankLines = RequireBlankLines::BetweenGroups, groups: Vec<RequireGroup> = vec![RequireGroup::Alias, RequireGroup::Relative, RequireGroup::Other] });

macro_rules! format_enum {
    ($name:ident, $default:ident, { $($variant:ident => $value:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
        #[serde(rename_all = "snake_case")]
        #[schemars(with = "String", inline, extend("enum" = [$($value),+]))]
        #[doc = concat!("Formatting setting ", stringify!($name), ".")]
        pub enum $name {
            $(#[doc = concat!("`", $value, "`.")]
              #[serde(rename = $value)] $variant),+
        }
        impl Default for $name { fn default() -> Self { Self::$default } }
    };
}

format_enum!(IndentStyle, Tabs, { Tabs => "tabs", Spaces => "spaces" });
format_enum!(LineEnding, Lf, { Lf => "lf", CrLf => "crlf" });
format_enum!(QuoteStyle, PreferDouble, { PreferDouble => "prefer_double", PreferSingle => "prefer_single", Double => "double", Single => "single", Preserve => "preserve" });
format_enum!(LeadingZero, Add, { Add => "add", Strip => "strip", Preserve => "preserve" });
format_enum!(Semicolons, Necessary, { Necessary => "necessary", Always => "always" });
format_enum!(BeforeFunctionParentheses, Never, { Never => "never", Calls => "calls", Definitions => "definitions", Always => "always" });
format_enum!(CallParentheses, Always, { Always => "always", OmitString => "omit_string", OmitTable => "omit_table", OmitLiteral => "omit_literal", Preserve => "preserve" });
format_enum!(Wrap, Preserve, { Auto => "auto", Preserve => "preserve", Always => "always", Never => "never" });
format_enum!(CallLayout, Vertical, { Vertical => "vertical", HugLast => "hug_last" });
format_enum!(TrailingComma, Multiline, { Multiline => "multiline", Never => "never" });
format_enum!(TableBlankLines, Preserve, { Preserve => "preserve", Remove => "remove" });
format_enum!(EdgeBlankLines, Remove, { Remove => "remove", Preserve => "preserve" });
format_enum!(SimpleBodies, Expand, { Expand => "expand", CompactFunctions => "compact_functions", CompactConditionals => "compact_conditionals", CompactAll => "compact_all" });
format_enum!(ChainLayout, Method, { Method => "method", Full => "full" });
format_enum!(IfExpressionLayout, Block, { Block => "block", Leading => "leading" });
format_enum!(IfExpressionPlacement, SameLine, { SameLine => "same_line", NextLine => "next_line" });
format_enum!(TypeTableSeparator, Comma, { Comma => "comma", Semicolon => "semicolon" });
format_enum!(RequireOrder, Grouped, { Grouped => "grouped", Alphabetical => "alphabetical", Preserve => "preserve" });
format_enum!(RequireBlankLines, BetweenGroups, { BetweenGroups => "between_groups", None => "none" });

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Group selection for static require paths.
pub enum RequireGroup {
    /// Paths beginning with `@`.
    Alias,

    /// Paths beginning with `./` or `../`.
    Relative,

    /// All remaining literal paths.
    #[default]
    Other,

    /// A named group with case-sensitive glob patterns.
    Custom {
        /// Group name.
        name: String,
        /// Path globs.
        patterns: Vec<String>,
    },
}

impl JsonSchema for RequireGroup {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "RequireGroup".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "anyOf": [
                { "type": "string", "enum": ["alias", "relative", "other"] },
                {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "patterns": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["name", "patterns"],
                    "additionalProperties": false
                }
            ]
        })
    }
}

impl<'de> Deserialize<'de> for RequireGroup {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum Input {
            Name(String),
            Custom { name: String, patterns: Vec<String> },
        }

        match Input::deserialize(deserializer)? {
            Input::Name(name) => match name.as_str() {
                "alias" => Ok(Self::Alias),
                "relative" => Ok(Self::Relative),
                "other" => Ok(Self::Other),

                _ => Err(serde::de::Error::custom(format!(
                    "unknown require group {name:?}"
                ))),
            },

            Input::Custom { name, patterns } => Ok(Self::Custom { name, patterns }),
        }
    }
}

impl Serialize for RequireGroup {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Alias => serializer.serialize_str("alias"),
            Self::Relative => serializer.serialize_str("relative"),
            Self::Other => serializer.serialize_str("other"),

            Self::Custom { name, patterns } => {
                use serde::ser::SerializeStruct;
                let mut state = serializer.serialize_struct("RequireGroup", 2)?;
                state.serialize_field("name", name)?;
                state.serialize_field("patterns", patterns)?;

                state.end()
            }
        }
    }
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            width: 120,
            indent_style: IndentStyle::Tabs,
            indent_width: 4,
            line_ending: LineEnding::Lf,
            final_newline: true,
            quote_style: QuoteStyle::PreferDouble,
            leading_zero: LeadingZero::Add,
            semicolons: Semicolons::Necessary,
            spacing: SpacingOptions::default(),
            calls: CallsOptions::default(),
            parameters: ParametersOptions::default(),
            tables: TablesOptions::default(),
            blocks: BlocksOptions::default(),
            chains: ChainsOptions::default(),
            if_expressions: IfExpressionsOptions::default(),
            types: TypesOptions::default(),
            requires: RequiresOptions::default(),
        }
    }
}

impl FormatOptions {
    pub(crate) fn merge(&mut self, layer: &FormatConfig) -> io::Result<()> {
        macro_rules! set {
            ($dst:expr, $src:expr) => {
                if let Some(value) = $src {
                    $dst = value;
                }
            };
        }

        set!(self.width, layer.width);
        set!(self.indent_style, layer.indent_style);
        set!(self.indent_width, layer.indent_width);
        set!(self.line_ending, layer.line_ending);
        set!(self.final_newline, layer.final_newline);
        set!(self.quote_style, layer.quote_style);
        set!(self.leading_zero, layer.leading_zero);
        set!(self.semicolons, layer.semicolons);

        if let Some(v) = &layer.spacing {
            set!(self.spacing.braces, v.braces);
            set!(self.spacing.interpolation, v.interpolation);
            set!(self.spacing.parentheses, v.parentheses);
            set!(self.spacing.brackets, v.brackets);

            set!(
                self.spacing.before_function_parentheses,
                v.before_function_parentheses
            );
        }

        if let Some(v) = &layer.calls {
            set!(self.calls.parentheses, v.parentheses);
            set!(self.calls.wrap, v.wrap);
            set!(self.calls.layout, v.layout);
        }

        if let Some(v) = &layer.parameters {
            set!(self.parameters.wrap, v.wrap);
        }

        if let Some(v) = &layer.tables {
            set!(self.tables.wrap, v.wrap);
            set!(self.tables.trailing_comma, v.trailing_comma);
            set!(self.tables.blank_lines, v.blank_lines);
        }

        if let Some(v) = &layer.blocks {
            set!(self.blocks.edge_blank_lines, v.edge_blank_lines);
            set!(self.blocks.simple_bodies, v.simple_bodies);
        }

        if let Some(v) = &layer.chains {
            set!(self.chains.wrap, v.wrap);
            set!(self.chains.layout, v.layout);
        }

        if let Some(v) = &layer.if_expressions {
            set!(self.if_expressions.wrap, v.wrap);
            set!(self.if_expressions.layout, v.layout);
            set!(self.if_expressions.placement, v.placement);
        }

        if let Some(v) = &layer.types {
            if let Some(tables) = &v.tables {
                set!(self.types.tables.wrap, tables.wrap);
                set!(self.types.tables.separator, tables.separator);
            }

            if let Some(operators) = &v.operators {
                set!(self.types.operators.wrap, operators.wrap);
            }
        }

        if let Some(v) = &layer.requires {
            set!(self.requires.order, v.order);
            set!(self.requires.blank_lines, v.blank_lines);

            if let Some(groups) = &v.groups {
                self.requires.groups.clone_from(groups);
            }
        }

        if self.width == 0 || self.indent_width == 0 {
            return Err(invalid("format width and indent_width must be positive"));
        }

        self.requires.validate()
    }
}

impl RequiresOptions {
    pub(crate) fn validate(&self) -> io::Result<()> {
        let mut names = std::collections::BTreeSet::new();

        for group in &self.groups {
            let (name, patterns) = match group {
                RequireGroup::Alias => ("alias", None),
                RequireGroup::Relative => ("relative", None),
                RequireGroup::Other => ("other", None),
                RequireGroup::Custom { name, patterns } => (name.as_str(), Some(patterns)),
            };

            if name.is_empty() || !names.insert(name) {
                return Err(invalid(format!(
                    "require group names must be nonempty and distinct: {name:?}"
                )));
            }

            if let Some(patterns) = patterns {
                if patterns.is_empty() {
                    return Err(invalid(format!(
                        "custom require group {name:?} needs patterns"
                    )));
                }

                for pattern in patterns {
                    if pattern.is_empty()
                        || pattern.contains('\0')
                        || glob::Pattern::new(pattern).is_err()
                    {
                        return Err(invalid(format!(
                            "invalid pattern {pattern:?} in require group {name:?}"
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

/// Roblox platform detection, API assets, and sourcemap settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RobloxConfig {
    /// Omission inherits or auto-detects Roblox from the effective sourcemaps.
    pub enabled: Option<bool>,

    /// Fetch supported values from Roblox Studio's `PCStudioApp` settings at startup.
    /// Omission inherits, then defaults to whether Roblox is effectively enabled.
    pub sync_flags: Option<bool>,

    /// API security level; defaults to normal game-script permissions (`none`).
    pub security: Option<Security>,

    /// Sourcemap files relative to this manifest; their source paths are relative to each map.
    /// Omission inherits the parent list, or discovers the nearest ancestor `sourcemap.json`.
    /// An explicit list replaces it, including `[]` to disable discovery.
    #[schemars(with = "Option<Vec<String>>")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sourcemaps: Option<Vec<PathBuf>>,
}

/// Security level of the published Roblox declarations.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Security {
    /// Normal game scripts.
    #[default]
    None,

    /// Local-user privileged APIs.
    Local,

    /// Studio plugin APIs.
    Plugin,

    /// Roblox-internal APIs.
    Roblox,
}

impl Security {
    pub(crate) fn file(self) -> &'static str {
        match self {
            Self::None => "none.d.luau",
            Self::Local => "local.d.luau",
            Self::Plugin => "plugin.d.luau",
            Self::Roblox => "roblox.d.luau",
        }
    }
}

/// Supported native Luau flag value types.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum FlagValue {
    /// Boolean value.
    Bool(bool),

    /// Signed 32-bit integer value.
    Int(i32),
}

/// Luau settings in the manifest's `snake_case` configuration format.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LuauConfig {
    /// Typechecking mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language_mode: Option<Mode>,

    /// Warning overrides, including the `*` wildcard.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub lint: BTreeMap<String, bool>,

    /// Whether lint warnings are errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lint_errors: Option<bool>,

    /// Whether type errors are reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_errors: Option<bool>,

    /// Additional globals. An explicit list replaces the inherited list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub globals: Option<Vec<String>>,

    /// Case-insensitive aliases; relative targets retain their defining directory.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub aliases: BTreeMap<String, String>,

    /// Declaration files keyed by documentation namespace (for example `@test`).
    /// Paths are relative to their manifest; child keys override inherited keys.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub definitions: BTreeMap<String, String>,

    /// JSON documentation paths or HTTPS URLs. Lists append through inheritance.
    /// Later files override matching keys; paths are relative to their manifest.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub documentation: Vec<String>,

    /// Explicit process-global bool or integer Luau flag values applied after synced values.
    /// Child keys override inherited keys; values are shared across projects in one process.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, FlagValue>,
}

/// Luau typechecking mode.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Skip typechecking.
    Nocheck,

    /// Infer types permissively.
    Nonstrict,

    /// Require strict type correctness.
    Strict,
}

impl LuauConfig {
    pub(super) fn merge(&mut self, layer: &Self) {
        if layer.language_mode.is_some() {
            self.language_mode = layer.language_mode;
        }

        if layer.lint_errors.is_some() {
            self.lint_errors = layer.lint_errors;
        }

        if layer.type_errors.is_some() {
            self.type_errors = layer.type_errors;
        }

        if layer.globals.is_some() {
            self.globals.clone_from(&layer.globals);
        }

        if layer.lint.contains_key("*") {
            self.lint.clear();
        }

        self.lint.extend(layer.lint.clone());
        self.aliases.extend(layer.aliases.clone());
        self.definitions.extend(layer.definitions.clone());

        self.flags.extend(layer.flags.clone());

        self.documentation
            .extend(layer.documentation.iter().cloned());
    }

    pub(super) fn validate(&mut self) -> io::Result<()> {
        for (package, path) in &self.definitions {
            if !package.strip_prefix('@').is_some_and(|name| {
                !name.is_empty()
                    && name
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            }) {
                return Err(invalid(format!("invalid definition namespace {package:?}")));
            }

            if path.is_empty() || path.contains('\0') {
                return Err(invalid(format!("invalid declaration path for {package:?}")));
            }
        }

        let mut aliases = BTreeMap::new();

        for (key, value) in std::mem::take(&mut self.aliases) {
            if key.is_empty()
                || key == "."
                || key == ".."
                || !key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                return Err(invalid(format!("invalid alias {key:?}")));
            }

            let name = key.to_ascii_lowercase();

            if name == "self" {
                return Err(invalid("the alias @self is reserved"));
            }

            if value.is_empty() || value.contains('\0') {
                return Err(invalid(format!("invalid target for alias {key:?}")));
            }

            if aliases.insert(name, value).is_some() {
                return Err(invalid(format!("duplicate case-insensitive alias {key:?}")));
            }
        }

        self.aliases = aliases;
        // Native validation covers warning names and keeps the JSON contract authoritative.
        instar_bridge::Configuration::new(self.native_json()?.as_bytes())?;

        Ok(())
    }

    pub(super) fn native_json(&self) -> io::Result<String> {
        let mut value = serde_json::to_value(self)?;

        if let Value::Object(object) = &mut value {
            object.remove("definitions");
            object.remove("documentation");
            object.remove("flags");

            for (from, to) in [
                ("language_mode", "languageMode"),
                ("lint_errors", "lintErrors"),
                ("type_errors", "typeErrors"),
            ] {
                if let Some(value) = object.remove(from) {
                    object.insert(to.to_owned(), value);
                }
            }
        }

        serde_json::to_string(&value).map_err(|error| invalid(error.to_string()))
    }
}

pub(super) fn parse_json(source: &str) -> io::Result<LuauConfig> {
    let options = jsonc_parser::ParseOptions {
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
        ..Default::default()
    };

    let value: Value = jsonc_parser::parse_to_serde_value(source, &options)
        .map_err(|error| invalid(error.to_string()))?;

    let Value::Object(object) = value else {
        return Err(invalid("configuration must be an object"));
    };

    legacy_config(object, ["languageMode", "lintErrors", "typeErrors"])
}

fn legacy_config(mut object: Map<String, Value>, keys: [&str; 3]) -> io::Result<LuauConfig> {
    for (from, to) in keys
        .into_iter()
        .zip(["language_mode", "lint_errors", "type_errors"])
    {
        if object.contains_key(to) {
            return Err(invalid(format!(
                "unknown legacy setting {to:?}; use {from:?}"
            )));
        }

        if let Some(value) = object.remove(from) {
            object.insert(to.to_owned(), value);
        }
    }

    serde_json::from_value(Value::Object(object)).map_err(|error| invalid(error.to_string()))
}

pub(super) fn parse_luau(source: &str) -> io::Result<LuauConfig> {
    let tree = vermis::parse(source.as_bytes());

    if let Some(error) = tree.diagnostics.first() {
        return Err(invalid(format!(
            "{} at byte {}",
            error.message, error.span.start
        )));
    }

    let NodeKind::Root { block, .. } = tree.node(tree.root).kind else {
        return Err(invalid("expected configuration block"));
    };

    let NodeKind::Block { statements } = &tree.node(block).kind else {
        return Err(invalid("expected configuration statements"));
    };

    let mut locals = BTreeMap::new();
    let mut returned = None;

    for statement in tree.list(statements) {
        if returned.is_some() {
            return Err(invalid("unexpected statement after config return"));
        }

        match &tree.node(statement.node).kind {
            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => {
                let values = tree
                    .list(values)
                    .iter()
                    .map(|value| literal(&tree, value.node, &locals))
                    .collect::<io::Result<Vec<_>>>()?;

                for (index, binding) in tree.list(bindings).iter().enumerate() {
                    let NodeKind::Binding { name, .. } = tree.node(binding.node).kind else {
                        return Err(invalid("invalid config binding"));
                    };

                    let name =
                        std::str::from_utf8(tree.text(name)).map_err(|e| invalid(e.to_string()))?;

                    locals.insert(
                        name.to_owned(),
                        values.get(index).cloned().unwrap_or(Value::Null),
                    );
                }
            }

            NodeKind::Return { values, .. } => {
                let mut values = tree.list(values).iter();

                let value = values
                    .next()
                    .ok_or_else(|| invalid("config must return a table"))?;

                if values.next().is_some() {
                    return Err(invalid("config must return exactly one table"));
                }

                returned = Some(literal(&tree, value.node, &locals)?);
            }

            _ => {
                return Err(invalid(format!(
                    "configuration must be declarative; unsupported statement at byte {}",
                    tree.node(statement.node).span.start
                )));
            }
        }
    }

    let value = returned.ok_or_else(|| invalid("configuration must return a table"))?;

    let object = value
        .as_object()
        .ok_or_else(|| invalid("configuration must return a table"))?;

    let Some(luau) = object.get("luau") else {
        return Ok(LuauConfig::default());
    };

    let mut luau = luau
        .as_object()
        .ok_or_else(|| invalid("luau must be a table"))?
        .clone();

    if luau
        .get("globals")
        .is_some_and(|v| v.as_object().is_some_and(Map::is_empty))
    {
        luau.insert("globals".to_owned(), Value::Array(Vec::new()));
    }

    legacy_config(luau, ["languagemode", "linterrors", "typeerrors"])
}

fn literal(
    tree: &Tree<'_>,
    node: NodeIndex,
    locals: &BTreeMap<String, Value>,
) -> io::Result<Value> {
    match &tree.node(node).kind {
        NodeKind::String { .. } => return string_value(tree.text(node)).map(Value::String),
        NodeKind::Boolean { .. } => return Ok(Value::Bool(tree.text(node) == b"true")),
        NodeKind::Nil { .. } => return Ok(Value::Null),

        NodeKind::Name { .. } => {
            let name = std::str::from_utf8(tree.text(node)).map_err(|e| invalid(e.to_string()))?;

            return locals
                .get(name)
                .cloned()
                .ok_or_else(|| invalid(format!("unknown config constant {name}")));
        }

        NodeKind::Number { .. } => {
            let text = std::str::from_utf8(tree.text(node)).map_err(|e| invalid(e.to_string()))?;

            return serde_json::from_str(text).map_err(|e| invalid(e.to_string()));
        }

        _ => {}
    }

    match &tree.node(node).kind {
        NodeKind::Group { expression, .. } | NodeKind::Assertion { expression, .. } => {
            literal(tree, *expression, locals)
        }

        NodeKind::Binary {
            left,
            operator,
            right,
        } if tree.token(*operator).span.bytes(tree.source) == b".." => {
            let left = literal(tree, *left, locals)?;
            let right = literal(tree, *right, locals)?;

            match (left, right) {
                (Value::String(left), Value::String(right)) => Ok(Value::String(left + &right)),
                _ => Err(invalid("config concatenation requires strings")),
            }
        }

        NodeKind::Table { fields, .. } => {
            let mut object = Map::new();
            let mut array = BTreeMap::new();
            let mut next_index = 1_u64;

            for field in tree.list(fields) {
                let NodeKind::TableField {
                    key,
                    value,
                    opening,
                    ..
                } = tree.node(field.node).kind
                else {
                    return Err(invalid("invalid config table field"));
                };

                let value = literal(tree, value, locals)?;

                let key = match key {
                    Some(key) if opening.is_none() => Value::String(
                        String::from_utf8(tree.text(key).to_vec())
                            .map_err(|e| invalid(e.to_string()))?,
                    ),

                    Some(key) => literal(tree, key, locals)?,

                    None => {
                        let key = Value::from(next_index);
                        next_index += 1;

                        key
                    }
                };

                match key {
                    Value::String(key) => {
                        object.insert(key, value);
                    }

                    Value::Number(key) => {
                        let key = key.as_u64().filter(|&n| n > 0).ok_or_else(|| {
                            invalid("config array index must be a positive integer")
                        })?;

                        array.insert(key, value);
                    }

                    _ => {
                        return Err(invalid(
                            "config table key must be a string or positive integer",
                        ));
                    }
                }
            }

            if array.is_empty() {
                return Ok(Value::Object(object));
            }

            if !object.is_empty() {
                return Err(invalid("mixed config tables are not supported"));
            }

            if array
                .keys()
                .copied()
                .ne(1..=u64::try_from(array.len()).map_err(|e| invalid(e.to_string()))?)
            {
                return Err(invalid("config arrays must be contiguous"));
            }

            Ok(Value::Array(array.into_values().collect()))
        }

        _ => Err(invalid(format!(
            "configuration must be declarative; unsupported expression at byte {}",
            tree.node(node).span.start
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_fields_preserve_omission_and_explicit_empty_lists() {
        for table in ["", "check", "format", "lint", "lsp.index", "lsp.imports"] {
            let header = if table.is_empty() {
                String::new()
            } else {
                format!("[{table}]\n")
            };

            for fields in [
                "",
                "include = []",
                "exclude = []",
                "include = []\nexclude = []",
            ] {
                let config: Config = toml::from_str(&format!("{header}{fields}")).unwrap();
                let serialized = serde_json::to_value(&config).unwrap();
                let mut selection = &serialized;

                for part in table.split('.').filter(|part| !part.is_empty()) {
                    selection = &selection[part];
                }

                for field in ["include", "exclude"] {
                    if fields.contains(field) {
                        assert_eq!(selection[field], serde_json::json!([]), "{table}.{field}");
                    } else {
                        assert!(selection.get(field).is_none(), "{table}.{field}");
                    }
                }
            }
        }
    }

    #[test]
    fn filter_schema_describes_optional_arrays_without_clearing_defaults() {
        for schema in [
            schemars::schema_for!(Config),
            schemars::schema_for!(FileFilter),
            schemars::schema_for!(LintConfig),
            schemars::schema_for!(FormatConfig),
            schemars::schema_for!(ImportsConfig),
        ] {
            let schema = serde_json::to_value(schema).unwrap();

            for field in ["include", "exclude"] {
                assert_eq!(schema["properties"][field]["type"], "array");
                assert!(schema["properties"][field].get("default").is_none());

                assert!(
                    schema["required"]
                        .as_array()
                        .is_none_or(|required| !required.iter().any(|name| name == field))
                );
            }
        }

        let defaults = format_schema_default();
        let lint = serde_json::to_value(lint_schema_default()).unwrap();

        for field in ["include", "exclude"] {
            assert!(defaults.get(field).is_none());
            assert!(lint.get(field).is_none());
        }
    }

    #[test]
    fn nested_type_formatting_partials_preserve_siblings() {
        let parent: Config = toml::from_str(
            r#"
            [format.types.tables]
            wrap = "always"
            separator = "semicolon"
            [format.types.operators]
            wrap = "always"
            "#,
        )
        .unwrap();

        let mut options = FormatOptions::default();
        options.merge(&parent.format).unwrap();
        assert_eq!(options.types.tables.wrap, Wrap::Always);

        assert_eq!(
            options.types.tables.separator,
            TypeTableSeparator::Semicolon
        );

        assert_eq!(options.types.operators.wrap, Wrap::Always);

        let child: Config = toml::from_str("[format.types.tables]\nwrap = \"never\"").unwrap();

        options.merge(&child.format).unwrap();
        assert_eq!(options.types.tables.wrap, Wrap::Never);

        assert_eq!(
            options.types.tables.separator,
            TypeTableSeparator::Semicolon
        );

        assert_eq!(options.types.operators.wrap, Wrap::Always);

        let child: Config =
            toml::from_str("[format.types.operators]\nwrap = \"preserve\"").unwrap();

        options.merge(&child.format).unwrap();

        let empty: Config =
            toml::from_str("[format.types.tables]\n[format.types.operators]").unwrap();

        options.merge(&empty.format).unwrap();
        options.merge(&FormatConfig::default()).unwrap();
        assert_eq!(options.types.tables.wrap, Wrap::Never);

        assert_eq!(
            options.types.tables.separator,
            TypeTableSeparator::Semicolon
        );

        assert_eq!(options.types.operators.wrap, Wrap::Preserve);

        let serialized = serde_json::to_value(&options).unwrap();
        assert_eq!(serialized["types"]["tables"]["wrap"], "never");
        assert_eq!(serialized["types"]["tables"]["separator"], "semicolon");
        assert_eq!(serialized["types"]["operators"]["wrap"], "preserve");
        let defaults = format_schema_default();
        assert_eq!(defaults["types"]["tables"]["wrap"], "auto");
        assert_eq!(defaults["types"]["tables"]["separator"], "comma");
        assert_eq!(defaults["types"]["operators"]["wrap"], "auto");
    }

    #[test]
    fn luau_flag_maps_convert_merge_and_serialize() {
        let parent: Config = toml::from_str("[luau.flags]\nBoolean = true\nInteger = 42").unwrap();

        let mut settings = LuauConfig::from(parent.luau);
        assert_eq!(settings.flags["Boolean"], FlagValue::Bool(true));
        assert_eq!(settings.flags["Integer"], FlagValue::Int(42));

        let child: Config = toml::from_str("[luau.flags]\nBoolean = false\nAdded = -7").unwrap();

        settings.merge(&LuauConfig::from(child.luau));
        settings.merge(&LuauConfig::default());
        assert_eq!(settings.flags.len(), 3);
        assert_eq!(settings.flags["Boolean"], FlagValue::Bool(false));
        assert_eq!(settings.flags["Integer"], FlagValue::Int(42));
        assert_eq!(settings.flags["Added"], FlagValue::Int(-7));

        let serialized = serde_json::to_value(&settings).unwrap();

        assert_eq!(
            serialized["flags"],
            serde_json::json!({ "Boolean": false, "Integer": 42, "Added": -7 })
        );

        let native: Value = serde_json::from_str(&settings.native_json().unwrap()).unwrap();
        assert!(native.get("flags").is_none());
    }

    #[test]
    fn lint_defaults_and_rule_tables() {
        let defaults = LintConfig::default();
        assert_eq!(defaults.level("length_as_condition"), LintLevel::Deny);
        assert_eq!(defaults.level("zero_step_loop"), LintLevel::Deny);
        assert_eq!(defaults.level("unused_variable"), LintLevel::Warn);
        assert_eq!(defaults.level("prefer_const"), LintLevel::Allow);
        assert_eq!(defaults.level("implicit_any_parameter"), LintLevel::Allow);
        assert_eq!(defaults.level("imaginary_rule"), LintLevel::Allow);
        assert_eq!(defaults.native_level("ImaginaryWarning"), None);
        assert_eq!(defaults.native_overrides().count(), 0);

        let schema_defaults = lint_schema_default();

        for (warning, level) in schema_defaults.native_overrides() {
            assert_eq!(level, LintLevel::Warn);
            assert_eq!(defaults.native_level(warning), None);
        }

        let values = serde_json::to_value(&schema_defaults).unwrap();

        for (rule, settings) in values.as_object().unwrap() {
            let Some(level) = settings.get("level") else {
                continue;
            };

            let level: LintLevel = serde_json::from_value(level.clone()).unwrap();
            assert_eq!(defaults.level(rule), level, "{rule}");

            let config: Config =
                toml::from_str(&format!("[lint.{rule}]\nlevel = \"info\"")).unwrap();

            assert_eq!(config.lint.level(rule), LintLevel::Info, "{rule}");
        }

        assert!(!defaults.unused_variable.parameters());
        assert!(!defaults.unused_variable.loop_variables());
        assert_eq!(defaults.unused_variable.ignore_pattern(), "^_");
        assert_eq!(defaults.high_cyclomatic_complexity.maximum_complexity(), 40);
        assert!(!defaults.prefer_const.mutated_tables_stay_local());
        assert!(!defaults.deprecated.ambiguous_methods());

        let schema = serde_json::to_value(schemars::schema_for!(Config)).unwrap();
        assert_eq!(schema["properties"]["lint"]["default"], values);
    }

    #[test]
    fn lint_rule_levels_and_options_inherit() {
        let mut parent: Config = toml::from_str(
            r#"
            [lint]
            include = ["src/**"]
            exclude = ["vendor/**"]
            [lint.local_unused]
            level = "deny"
            [lint.format_string]
            level = "warn"
            [lint.unused_variable]
            level = "deny"
            parameters = true
            loop_variables = true
            ignore_pattern = "^skip"
            [lint.high_cyclomatic_complexity]
            level = "warn"
            maximum_complexity = 7
            [lint.prefer_const]
            level = "info"
            mutated_tables_stay_local = true
            [lint.deprecated]
            ambiguous_methods = true
            paths = { Old = "Original", Kept = "Replacement" }
            [lint.restricted_globals]
            level = "deny"
            names = { debug = "Original", shared = "Inherited" }
            [lint.restricted_module_paths]
            paths = { "./private" = "Original", "./kept" = "Inherited" }
            "#,
        )
        .unwrap();

        let child: Config = toml::from_str(
            r#"
            [lint]
            include = ["tests/**"]
            [lint.local_unused]
            level = "allow"
            [lint.multi_line_statement]
            level = "info"
            [lint.deprecated_api]
            level = "deny"
            [lint.unused_variable]
            parameters = false
            loop_variables = false
            ignore_pattern = "^_"
            [lint.high_cyclomatic_complexity]
            maximum_complexity = 40
            [lint.prefer_const]
            level = "allow"
            mutated_tables_stay_local = false
            [lint.deprecated]
            ambiguous_methods = false
            paths = { Old = "Updated", Added = "New" }
            [lint.restricted_globals]
            names = { debug = "Updated", forbidden = "New" }
            [lint.restricted_module_paths]
            paths = { "./private" = "Updated", "./added" = "New" }
            "#,
        )
        .unwrap();

        parent.lint.merge(&child.lint);
        parent.lint.merge(&LintConfig::default());
        let lint = &mut parent.lint;
        assert_eq!(lint.include, Some(vec!["tests/**".to_owned()]));
        assert_eq!(lint.exclude, Some(vec!["vendor/**".to_owned()]));
        assert_eq!(lint.native_level("LocalUnused"), Some(LintLevel::Allow));
        assert_eq!(lint.native_level("FormatString"), Some(LintLevel::Warn));

        assert_eq!(
            lint.native_level("MultiLineStatement"),
            Some(LintLevel::Info)
        );

        assert_eq!(lint.native_level("DeprecatedApi"), Some(LintLevel::Deny));
        assert_eq!(lint.native_overrides().count(), 4);
        assert_eq!(lint.level("local_unused"), LintLevel::Allow);
        assert_eq!(lint.level("unused_variable"), LintLevel::Deny);
        assert_eq!(lint.level("high_cyclomatic_complexity"), LintLevel::Warn);
        assert_eq!(lint.level("prefer_const"), LintLevel::Allow);
        assert_eq!(lint.level("restricted_globals"), LintLevel::Deny);
        assert!(!lint.unused_variable.parameters());
        assert!(!lint.unused_variable.loop_variables());
        assert_eq!(lint.unused_variable.ignore_pattern(), "^_");
        assert_eq!(lint.high_cyclomatic_complexity.maximum_complexity(), 40);
        assert!(!lint.prefer_const.mutated_tables_stay_local());
        assert!(!lint.deprecated.ambiguous_methods());

        for (actual, expected) in [
            (
                &lint.deprecated.paths,
                [
                    ("Old", "Updated"),
                    ("Kept", "Replacement"),
                    ("Added", "New"),
                ],
            ),
            (
                &lint.restricted_globals.names,
                [
                    ("debug", "Updated"),
                    ("shared", "Inherited"),
                    ("forbidden", "New"),
                ],
            ),
            (
                &lint.restricted_module_paths.paths,
                [
                    ("./private", "Updated"),
                    ("./kept", "Inherited"),
                    ("./added", "New"),
                ],
            ),
        ] {
            assert_eq!(
                actual,
                &expected
                    .into_iter()
                    .map(|(name, reason)| (name.to_owned(), reason.to_owned()))
                    .collect::<BTreeMap<_, _>>()
            );
        }

        lint.validate().unwrap();

        let child: LintConfig = toml::from_str("exclude = []").unwrap();
        lint.merge(&child);
        assert_eq!(lint.include, Some(vec!["tests/**".to_owned()]));
        assert_eq!(lint.exclude, Some(Vec::new()));

        let child: LintConfig = toml::from_str("include = []").unwrap();
        lint.merge(&child);
        assert_eq!(lint.include, Some(Vec::new()));
        assert_eq!(lint.exclude, Some(Vec::new()));
    }

    #[test]
    fn lint_rule_tables_validate_names_options_and_patterns() {
        for source in [
            "[lint.imaginary_rule]\nlevel = \"warn\"",
            "[lint.empty_if]\nparameters = true",
            "[lint.unused_variable]\nmaximum_complexity = 5",
            "[lint.restricted_globals]\npaths = {}",
        ] {
            let error = toml::from_str::<Config>(source).unwrap_err();
            assert!(error.to_string().contains("unknown field"), "{error}");
        }

        assert!(
            toml::from_str::<Config>("[lint.empty_if]\nlevel = \"fatal\"")
                .unwrap_err()
                .to_string()
                .contains("unknown variant")
        );

        let config: Config =
            toml::from_str("[lint.unused_variable]\nignore_pattern = \"[\"").unwrap();

        assert!(
            config
                .lint
                .validate()
                .unwrap_err()
                .to_string()
                .contains("invalid unused_variable.ignore_pattern")
        );
    }

    #[test]
    fn import_defaults_and_omitted_preferences_inherit() {
        let mut imports = Config::default().lsp.imports;
        assert_eq!(imports.require.unwrap_or_default(), RequireStyle::Instance);
        assert_eq!(imports.binding.unwrap_or_default(), BindingStyle::Local);

        let config: Config =
            toml::from_str("[lsp.imports]\nrequire = \"instance\"\nbinding = \"local\"").unwrap();

        assert_eq!(config.lsp.imports.require, Some(RequireStyle::Instance));
        assert_eq!(config.lsp.imports.binding, Some(BindingStyle::Local));

        let parent: Config = toml::from_str(
            "[lsp.imports]\nrequire = \"string\"\nbinding = \"const\"\n\
             include = [\"modules/**\"]\nexclude = [\"modules/private/**\"]",
        )
        .unwrap();

        imports.merge(&parent.lsp.imports);
        imports.merge(&ImportsConfig::default());
        assert_eq!(imports.require, Some(RequireStyle::String));
        assert_eq!(imports.binding, Some(BindingStyle::Const));
        assert_eq!(imports.include, Some(vec!["modules/**".to_owned()]));
        assert_eq!(imports.exclude, Some(vec!["modules/private/**".to_owned()]));

        let include_only: Config =
            toml::from_str("[lsp.imports]\ninclude = [\"extra/**\"]").unwrap();

        imports.merge(&include_only.lsp.imports);
        assert_eq!(imports.include, Some(vec!["extra/**".to_owned()]));
        assert_eq!(imports.exclude, Some(vec!["modules/private/**".to_owned()]));

        let child: Config = toml::from_str(
            "[lsp.imports]\nbinding = \"local\"\ninclude = []\nexclude = [\"generated/**\"]",
        )
        .unwrap();

        imports.merge(&child.lsp.imports);
        assert_eq!(imports.require, Some(RequireStyle::String));
        assert_eq!(imports.binding, Some(BindingStyle::Local));
        assert_eq!(imports.include, Some(Vec::new()));
        assert_eq!(imports.exclude, Some(vec!["generated/**".to_owned()]));

        let exclude_only: Config = toml::from_str("[lsp.imports]\nexclude = []").unwrap();
        imports.merge(&exclude_only.lsp.imports);
        imports.merge(&ImportsConfig::default());
        assert_eq!(imports.include, Some(Vec::new()));
        assert_eq!(imports.exclude, Some(Vec::new()));
        assert_eq!(imports.require, Some(RequireStyle::String));
        assert_eq!(imports.binding, Some(BindingStyle::Local));

        for (key, value) in [("require", "relative"), ("binding", "global")] {
            let error = toml::from_str::<Config>(&format!("[lsp.imports]\n{key} = \"{value}\""))
                .unwrap_err();

            assert!(error.to_string().contains("unknown variant"));
        }
    }
}
