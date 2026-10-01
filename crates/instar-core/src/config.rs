//! Project configuration formats, Luau settings, and formatting options.

use std::{collections::BTreeMap, io, path::PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use vermis::{Kind, Parts, View};

use crate::{invalid, string_value};

/// Lint configuration and file selection.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LintConfig {
    /// File include globs.
    pub include: Vec<String>,

    /// File exclude globs.
    pub exclude: Vec<String>,

    /// Whether native warnings are promoted to errors.
    #[schemars(with = "bool", extend("default" = false))]
    pub lint_errors: Option<bool>,

    /// Native warning switches by Luau warning name.
    pub luau: BTreeMap<String, bool>,

    /// Rule levels by name.
    pub rules: BTreeMap<String, LintLevel>,

    /// Group levels by name.
    pub groups: BTreeMap<String, LintLevel>,

    /// Rule detection options.
    #[schemars(extend("default" = lint_schema_default().options))]
    pub options: LintOptions,
}

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

/// Typed options for custom lint rules.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LintOptions {
    /// Unused binding checks.
    #[schemars(extend("default" = lint_schema_default().options.unused_variable))]
    pub unused_variable: UnusedVariableOptions,

    /// Maximum function branch score.
    #[schemars(extend("default" = lint_schema_default().options.high_cyclomatic_complexity))]
    pub high_cyclomatic_complexity: HighCyclomaticComplexityOptions,

    /// Constant-binding preferences.
    #[schemars(extend("default" = lint_schema_default().options.prefer_const))]
    pub prefer_const: PreferConstOptions,

    /// Project-specific deprecated API paths.
    #[schemars(extend("default" = lint_schema_default().options.deprecated))]
    pub deprecated: DeprecatedOptions,

    /// Forbidden global names and reasons.
    pub restricted_globals: BTreeMap<String, String>,

    /// Forbidden literal require paths.
    pub restricted_module_paths: RestrictedModulePathsOptions,
}

/// Optional unused-binding detection settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct UnusedVariableOptions {
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
    /// Deprecated API path to replacement mappings.
    pub additional: BTreeMap<String, String>,

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

/// Restricted module path settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct RestrictedModulePathsOptions {
    /// Forbidden literal require paths and reasons.
    pub paths: BTreeMap<String, String>,
}

/// Manifest Luau settings; native lint switches belong under `[lint]`.
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

    /// Process-global Luau flag settings.
    pub fflags: LuauFlagsConfig,
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
            fflags: config.fflags,
        }
    }
}

impl LintConfig {
    /// Merges an inherited lint layer, preserving explicit scalar overrides.
    pub fn merge(&mut self, layer: &Self) {
        self.include.extend(layer.include.iter().cloned());
        self.exclude.extend(layer.exclude.iter().cloned());

        if layer.lint_errors.is_some() {
            self.lint_errors = layer.lint_errors;
        }

        if layer.luau.contains_key("*") {
            self.luau.clear();
        }

        self.luau.extend(layer.luau.clone());
        self.rules.extend(layer.rules.clone());
        self.groups.extend(layer.groups.clone());

        self.options.unused_variable.parameters = layer
            .options
            .unused_variable
            .parameters
            .or(self.options.unused_variable.parameters);

        self.options.unused_variable.loop_variables = layer
            .options
            .unused_variable
            .loop_variables
            .or(self.options.unused_variable.loop_variables);

        if layer.options.unused_variable.ignore_pattern.is_some() {
            self.options
                .unused_variable
                .ignore_pattern
                .clone_from(&layer.options.unused_variable.ignore_pattern);
        }

        self.options.high_cyclomatic_complexity.maximum_complexity = layer
            .options
            .high_cyclomatic_complexity
            .maximum_complexity
            .or(self.options.high_cyclomatic_complexity.maximum_complexity);

        self.options.prefer_const.mutated_tables_stay_local = layer
            .options
            .prefer_const
            .mutated_tables_stay_local
            .or(self.options.prefer_const.mutated_tables_stay_local);

        self.options
            .deprecated
            .additional
            .extend(layer.options.deprecated.additional.clone());

        self.options.deprecated.ambiguous_methods = layer
            .options
            .deprecated
            .ambiguous_methods
            .or(self.options.deprecated.ambiguous_methods);

        self.options
            .restricted_globals
            .extend(layer.options.restricted_globals.clone());

        self.options
            .restricted_module_paths
            .paths
            .extend(layer.options.restricted_module_paths.paths.clone());
    }

    /// Resolves the effective level for a custom lint rule.
    #[must_use]
    pub fn level(&self, rule: &str) -> LintLevel {
        if let Some(level) = self.rules.get(rule) {
            return *level;
        }

        let Some(&(_, group, default)) = RULES.iter().find(|(name, _, _)| *name == rule) else {
            return LintLevel::Allow;
        };

        if default != LintLevel::Allow
            && let Some(level) = self.groups.get(group)
        {
            return *level;
        }

        default
    }

    /// Validates custom lint rule names, groups, and option patterns.
    ///
    /// # Errors
    /// Returns an error for unknown rules/groups or an invalid pattern.
    pub fn validate(&self) -> io::Result<()> {
        for rule in self.rules.keys() {
            if !RULES.iter().any(|(name, _, _)| name == rule) {
                return Err(invalid(format!("unknown lint rule {rule:?}")));
            }
        }

        for group in self.groups.keys() {
            if !GROUPS.contains(&group.as_str()) {
                return Err(invalid(format!("unknown lint group {group:?}")));
            }
        }

        if let Some(pattern) = &self.options.unused_variable.ignore_pattern {
            regex::Regex::new(pattern).map_err(|error| {
                invalid(format!("invalid unused_variable.ignore_pattern: {error}"))
            })?;
        }

        Ok(())
    }
}

fn lint_schema_default() -> LintConfig {
    LintConfig {
        lint_errors: Some(false),
        options: LintOptions {
            unused_variable: UnusedVariableOptions {
                parameters: Some(false),
                loop_variables: Some(false),
                ignore_pattern: Some("^_".to_owned()),
            },
            high_cyclomatic_complexity: HighCyclomaticComplexityOptions {
                maximum_complexity: Some(40),
            },
            prefer_const: PreferConstOptions {
                mutated_tables_stay_local: Some(false),
            },
            deprecated: DeprecatedOptions {
                ambiguous_methods: Some(false),
                ..DeprecatedOptions::default()
            },
            ..LintOptions::default()
        },
        ..LintConfig::default()
    }
}

const GROUPS: &[&str] = &[
    "correctness",
    "suspicious",
    "style",
    "complexity",
    "performance",
    "roblox",
];

const RULES: &[(&str, &str, LintLevel)] = &[
    ("almost_swapped", "correctness", LintLevel::Warn),
    ("bad_string_escape", "correctness", LintLevel::Warn),
    ("compare_nan", "correctness", LintLevel::Warn),
    ("constant_condition", "correctness", LintLevel::Warn),
    ("constant_table_comparison", "correctness", LintLevel::Warn),
    ("length_as_condition", "correctness", LintLevel::Deny),
    ("mismatched_arg_count", "correctness", LintLevel::Warn),
    ("must_use", "correctness", LintLevel::Warn),
    ("unused_variable", "correctness", LintLevel::Warn),
    ("type_check_inside_call", "correctness", LintLevel::Warn),
    ("zero_step_loop", "correctness", LintLevel::Deny),
    ("divide_by_zero", "suspicious", LintLevel::Warn),
    ("empty_if", "suspicious", LintLevel::Warn),
    ("empty_loop", "suspicious", LintLevel::Warn),
    ("global_usage", "suspicious", LintLevel::Warn),
    ("if_same_then_else", "suspicious", LintLevel::Warn),
    ("ignored_pcall_result", "suspicious", LintLevel::Warn),
    ("implicit_any_local", "suspicious", LintLevel::Warn),
    ("implicit_any_parameter", "suspicious", LintLevel::Allow),
    ("mixed_table", "suspicious", LintLevel::Warn),
    ("self_assignment", "suspicious", LintLevel::Warn),
    ("unscoped_variables", "suspicious", LintLevel::Warn),
    ("and_or_conditional", "style", LintLevel::Allow),
    ("collapsible_if", "style", LintLevel::Allow),
    ("deprecated", "style", LintLevel::Warn),
    ("else_after_return", "style", LintLevel::Allow),
    ("if_expression_assignment", "style", LintLevel::Allow),
    ("negated_condition", "style", LintLevel::Allow),
    ("non_const_require", "style", LintLevel::Allow),
    ("parenthese_conditions", "style", LintLevel::Warn),
    ("prefer_const", "style", LintLevel::Allow),
    ("restricted_globals", "style", LintLevel::Warn),
    ("restricted_module_paths", "style", LintLevel::Warn),
    ("high_cyclomatic_complexity", "complexity", LintLevel::Allow),
    ("loop_invariant_call", "performance", LintLevel::Warn),
    ("manual_table_clone", "performance", LintLevel::Warn),
    ("string_concat_in_loop", "performance", LintLevel::Warn),
    (
        "roblox_incorrect_color3_new_bounds",
        "roblox",
        LintLevel::Warn,
    ),
    (
        "roblox_manual_fromscale_or_fromoffset",
        "roblox",
        LintLevel::Warn,
    ),
    ("roblox_prefer_get_players", "roblox", LintLevel::Warn),
    ("roblox_suspicious_udim2_new", "roblox", LintLevel::Warn),
];

/// Project manifest. Luau settings live in `[luau]`; formatting settings in `[format]`.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Global include globs, relative to this manifest; inherited lists append.
    /// An empty effective list allows every path.
    pub include: Vec<String>,

    /// Global exclude globs; exclusions always win.
    pub exclude: Vec<String>,

    /// Additional file selection for analysis.
    pub analyze: FileFilter,

    /// Formatting style and additional file selection.
    #[schemars(extend("default" = format_schema_default()))]
    pub format: FormatConfig,

    /// Lint rules, options, and additional file selection.
    #[schemars(extend("default" = lint_schema_default()))]
    pub lint: LintConfig,

    /// Additional file selection for the language server.
    pub lsp: FileFilter,

    /// Luau analysis, resolution, and process-global fast-flag settings.
    pub luau: ManifestLuauConfig,

    /// Optional Roblox sourcemap configuration.
    pub roblox: RobloxConfig,
}

/// Service-specific globs, intersected with global selection.
/// Lists append through inheritance; `[]` adds nothing.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct FileFilter {
    /// Include globs relative to their manifest; an empty effective list allows every path.
    pub include: Vec<String>,

    /// Exclude globs relative to their manifest; exclusions always win.
    pub exclude: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
/// Optional formatting overrides and file selection. Omitted values inherit;
/// schema defaults describe the root configuration.
pub struct FormatConfig {
    /// Additional include globs for formatting.
    pub include: Vec<String>,

    /// Exclude globs for formatting.
    pub exclude: Vec<String>,

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
    table_wrap: Wrap => "Wrap",
    operator_wrap: Wrap => "Wrap",
    table_separator: TypeTableSeparator => "TypeTableSeparator"
});

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
    table_wrap: Wrap = Wrap::Auto,
    operator_wrap: Wrap = Wrap::Auto,
    table_separator: TypeTableSeparator = TypeTableSeparator::Comma
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
            set!(self.types.table_wrap, v.table_wrap);
            set!(self.types.operator_wrap, v.operator_wrap);
            set!(self.types.table_separator, v.table_separator);
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

/// Process-global Luau flag settings shared across projects in one process.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LuauFlagsConfig {
    /// Fetch supported values from Roblox Studio's `PCStudioApp` settings at startup.
    pub sync_roblox: Option<bool>,

    /// Explicit bool or integer values applied after synced values.
    pub overrides: BTreeMap<String, FlagValue>,
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

    /// Process-global Luau flag settings.
    pub fflags: LuauFlagsConfig,
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

        if let Some(sync) = layer.fflags.sync_roblox {
            self.fflags.sync_roblox = Some(sync);
        }

        self.fflags.overrides.extend(layer.fflags.overrides.clone());

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
            object.remove("fflags");

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
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
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

    let root = tree
        .view(tree.root)
        .ok_or_else(|| invalid("missing configuration root"))?;

    let Some(Parts::Root { block }) = root.parts() else {
        return Err(invalid("expected configuration block"));
    };

    let mut locals = BTreeMap::new();
    let mut returned = None;

    for statement in block.children() {
        if returned.is_some() {
            return Err(invalid("unexpected statement after config return"));
        }

        match statement.parts() {
            Some(Parts::Local { bindings, values }) => {
                let values = values
                    .map(|value| literal(value, &locals))
                    .collect::<io::Result<Vec<_>>>()?;

                for (index, binding) in bindings.enumerate() {
                    let Some(Parts::Binding { name, .. }) = binding.parts() else {
                        return Err(invalid("invalid config binding"));
                    };

                    let name =
                        std::str::from_utf8(name.text()).map_err(|e| invalid(e.to_string()))?;

                    locals.insert(
                        name.to_owned(),
                        values.get(index).cloned().unwrap_or(Value::Null),
                    );
                }
            }

            Some(Parts::Return { mut values }) => {
                let value = values
                    .next()
                    .ok_or_else(|| invalid("config must return a table"))?;

                if values.next().is_some() {
                    return Err(invalid("config must return exactly one table"));
                }

                returned = Some(literal(value, &locals)?);
            }

            _ => {
                return Err(invalid(format!(
                    "configuration must be declarative; unsupported statement at byte {}",
                    statement.span().start
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

fn literal(node: View<'_, '_>, locals: &BTreeMap<String, Value>) -> io::Result<Value> {
    match node.kind() {
        Kind::String => return string_value(node.text()).map(Value::String),
        Kind::Boolean => return Ok(Value::Bool(node.text() == b"true")),
        Kind::Nil => return Ok(Value::Null),

        Kind::Name => {
            let name = std::str::from_utf8(node.text()).map_err(|e| invalid(e.to_string()))?;

            return locals
                .get(name)
                .cloned()
                .ok_or_else(|| invalid(format!("unknown config constant {name}")));
        }

        Kind::Number => {
            let text = std::str::from_utf8(node.text()).map_err(|e| invalid(e.to_string()))?;

            return serde_json::from_str(text).map_err(|e| invalid(e.to_string()));
        }

        _ => {}
    }

    match node.parts() {
        Some(Parts::Group { expression } | Parts::Assertion { expression, .. }) => {
            literal(expression, locals)
        }

        Some(Parts::Binary {
            left,
            operator,
            right,
        }) if operator.text() == b".." => {
            let left = literal(left, locals)?;
            let right = literal(right, locals)?;

            match (left, right) {
                (Value::String(left), Value::String(right)) => Ok(Value::String(left + &right)),
                _ => Err(invalid("config concatenation requires strings")),
            }
        }

        Some(Parts::Table { fields }) => {
            let mut object = Map::new();
            let mut array = BTreeMap::new();
            let mut next_index = 1_u64;

            for field in fields {
                let Some(Parts::TableField {
                    key,
                    value,
                    indexed,
                }) = field.parts()
                else {
                    return Err(invalid("invalid config table field"));
                };

                let value = literal(value, locals)?;

                let key = match key {
                    Some(key) if !indexed => Value::String(
                        String::from_utf8(key.text().to_vec())
                            .map_err(|e| invalid(e.to_string()))?,
                    ),

                    Some(key) => literal(key, locals)?,

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
            node.span().start
        ))),
    }
}
