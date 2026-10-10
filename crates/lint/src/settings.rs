use std::{collections::BTreeMap, io};

use instar_luau::UpstreamWarningIdentity;
use instar_shared::{Availability, Severity, error::invalid};
use instar_syntax::identifier;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Preset, RuleDefinition, RuleIdentity, RuleProducer, rules,
    rules::{
        bindings::{DeprecatedSymbols, ImmutableBindings, RestrictedGlobals, UnusedBindings},
        calls::RestrictedImports,
    },
};

/// Parameterless rule settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasicSettings {}

/// Typed parameters for each catalog identity.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "rule", content = "parameters")]
pub enum RuleSettings {
    #[serde(rename = "almost_swapped")]
    /// Parameters for `AlmostSwapped`.
    AlmostSwapped(BasicSettings),

    #[serde(rename = "bad_string_escape")]
    /// Parameters for `BadStringEscape`.
    BadStringEscape(BasicSettings),

    #[serde(rename = "compare_nan")]
    /// Parameters for `CompareNan`.
    CompareNan(BasicSettings),

    #[serde(rename = "constant_condition")]
    /// Parameters for `ConstantCondition`.
    ConstantCondition(BasicSettings),

    #[serde(rename = "constant_table_comparison")]
    /// Parameters for `ConstantTableComparison`.
    ConstantTableComparison(BasicSettings),

    #[serde(rename = "length_as_condition")]
    /// Parameters for `LengthAsCondition`.
    LengthAsCondition(BasicSettings),

    #[serde(rename = "mismatched_argument_count")]
    /// Parameters for `MismatchedArgumentCount`.
    MismatchedArgumentCount(BasicSettings),

    #[serde(rename = "must_use")]
    /// Parameters for `MustUse`.
    MustUse(BasicSettings),

    #[serde(rename = "zero_step_loop")]
    /// Parameters for `ZeroStepLoop`.
    ZeroStepLoop(BasicSettings),

    #[serde(rename = "divide_by_zero")]
    /// Parameters for `DivideByZero`.
    DivideByZero(BasicSettings),

    #[serde(rename = "self_assignment")]
    /// Parameters for `SelfAssignment`.
    SelfAssignment(BasicSettings),

    #[serde(rename = "empty_if")]
    /// Parameters for `EmptyIf`.
    EmptyIf(BasicSettings),

    #[serde(rename = "empty_loop")]
    /// Parameters for `EmptyLoop`.
    EmptyLoop(BasicSettings),

    #[serde(rename = "if_same_then_else")]
    /// Parameters for `IfSameThenElse`.
    IfSameThenElse(BasicSettings),

    #[serde(rename = "ignored_pcall_result")]
    /// Parameters for `IgnoredPcallResult`.
    IgnoredPcallResult(BasicSettings),

    #[serde(rename = "mixed_table")]
    /// Parameters for `MixedTable`.
    MixedTable(BasicSettings),

    #[serde(rename = "unscoped_variables")]
    /// Parameters for `UnscopedVariables`.
    UnscopedVariables(BasicSettings),

    #[serde(rename = "roblox_incorrect_color3_new_bounds")]
    /// Parameters for `RobloxIncorrectColor3NewBounds`.
    RobloxIncorrectColor3NewBounds(BasicSettings),

    #[serde(rename = "roblox_manual_fromscale_or_fromoffset")]
    /// Parameters for `RobloxManualFromscaleOrFromoffset`.
    RobloxManualFromscaleOrFromoffset(BasicSettings),

    #[serde(rename = "roblox_prefer_get_players")]
    /// Parameters for `RobloxPreferGetPlayers`.
    RobloxPreferGetPlayers(BasicSettings),

    #[serde(rename = "roblox_suspicious_udim2_new")]
    /// Parameters for `RobloxSuspiciousUdim2New`.
    RobloxSuspiciousUdim2New(BasicSettings),

    #[serde(rename = "implicit_any_local")]
    /// Parameters for `ImplicitAnyLocal`.
    ImplicitAnyLocal(BasicSettings),

    #[serde(rename = "implicit_any_parameter")]
    /// Parameters for `ImplicitAnyParameter`.
    ImplicitAnyParameter(BasicSettings),

    #[serde(rename = "and_or_conditional")]
    /// Parameters for `AndOrConditional`.
    AndOrConditional(BasicSettings),

    #[serde(rename = "collapsible_if")]
    /// Parameters for `CollapsibleIf`.
    CollapsibleIf(BasicSettings),

    #[serde(rename = "else_after_return")]
    /// Parameters for `ElseAfterReturn`.
    ElseAfterReturn(BasicSettings),

    #[serde(rename = "if_expression_assignment")]
    /// Parameters for `IfExpressionAssignment`.
    IfExpressionAssignment(BasicSettings),

    #[serde(rename = "negated_condition")]
    /// Parameters for `NegatedCondition`.
    NegatedCondition(BasicSettings),

    #[serde(rename = "non_const_require")]
    /// Parameters for `NonConstRequire`.
    NonConstRequire(BasicSettings),

    #[serde(rename = "parenthesized_conditions")]
    /// Parameters for `ParenthesizedConditions`.
    ParenthesizedConditions(BasicSettings),

    #[serde(rename = "global_usage")]
    /// Parameters for `GlobalUsage`.
    GlobalUsage(BasicSettings),

    #[serde(rename = "type_check_inside_call")]
    /// Parameters for `TypeCheckInsideCall`.
    TypeCheckInsideCall(BasicSettings),

    #[serde(rename = "loop_invariant_call")]
    /// Parameters for `LoopInvariantCall`.
    LoopInvariantCall(BasicSettings),

    #[serde(rename = "manual_table_clone")]
    /// Parameters for `ManualTableClone`.
    ManualTableClone(BasicSettings),

    #[serde(rename = "string_concat_in_loop")]
    /// Parameters for `StringConcatInLoop`.
    StringConcatInLoop(BasicSettings),

    #[serde(rename = "unused_variable")]
    /// Parameters for `UnusedVariable`.
    UnusedVariable(UnusedBindings),

    #[serde(rename = "prefer_const")]
    /// Parameters for `PreferConst`.
    PreferConst(ImmutableBindings),

    #[serde(rename = "deprecated")]
    /// Parameters for `Deprecated`.
    Deprecated(DeprecatedSymbols),

    #[serde(rename = "restricted_globals")]
    /// Parameters for `RestrictedGlobals`.
    RestrictedGlobals(RestrictedGlobals),

    #[serde(rename = "restricted_module_paths")]
    /// Parameters for `RestrictedModulePaths`.
    RestrictedModulePaths(RestrictedImports),

    /// Parameters for `UnknownGlobal`.
    UnknownGlobal(BasicSettings),

    /// Parameters for `DeprecatedGlobal`.
    DeprecatedGlobal(BasicSettings),

    /// Parameters for `GlobalUsedAsLocal`.
    GlobalUsedAsLocal(BasicSettings),

    /// Parameters for `LocalShadow`.
    LocalShadow(BasicSettings),

    /// Parameters for `SameLineStatement`.
    SameLineStatement(BasicSettings),

    /// Parameters for `MultiLineStatement`.
    MultiLineStatement(BasicSettings),

    /// Parameters for `LocalUnused`.
    LocalUnused(BasicSettings),

    /// Parameters for `FunctionUnused`.
    FunctionUnused(BasicSettings),

    /// Parameters for `ImportUnused`.
    ImportUnused(BasicSettings),

    /// Parameters for `BuiltinGlobalWrite`.
    BuiltinGlobalWrite(BasicSettings),

    /// Parameters for `PlaceholderRead`.
    PlaceholderRead(BasicSettings),

    /// Parameters for `UnreachableCode`.
    UnreachableCode(BasicSettings),

    /// Parameters for `UnknownType`.
    UnknownType(BasicSettings),

    /// Parameters for `ForRange`.
    ForRange(BasicSettings),

    /// Parameters for `UnbalancedAssignment`.
    UnbalancedAssignment(BasicSettings),

    /// Parameters for `ImplicitReturn`.
    ImplicitReturn(BasicSettings),

    /// Parameters for `DuplicateLocal`.
    DuplicateLocal(BasicSettings),

    /// Parameters for `FormatString`.
    FormatString(BasicSettings),

    /// Parameters for `TableLiteral`.
    TableLiteral(BasicSettings),

    /// Parameters for `UninitializedLocal`.
    UninitializedLocal(BasicSettings),

    /// Parameters for `DuplicateFunction`.
    DuplicateFunction(BasicSettings),

    /// Parameters for `DeprecatedApi`.
    DeprecatedApi(BasicSettings),

    /// Parameters for `TableOperations`.
    TableOperations(BasicSettings),

    /// Parameters for `DuplicateCondition`.
    DuplicateCondition(BasicSettings),

    /// Parameters for `MisleadingAndOr`.
    MisleadingAndOr(BasicSettings),

    /// Parameters for `CommentDirective`.
    CommentDirective(BasicSettings),

    /// Parameters for `IntegerParsing`.
    IntegerParsing(BasicSettings),

    /// Parameters for `ComparisonPrecedence`.
    ComparisonPrecedence(BasicSettings),

    /// Parameters for `RedundantNativeAttribute`.
    RedundantNativeAttribute(BasicSettings),
}

impl RuleSettings {
    /// Constructs default typed parameters for a catalog identity.
    #[must_use]
    pub fn for_rule(rule: RuleIdentity) -> Self {
        match rule {
            RuleIdentity::AlmostSwapped => Self::basic(Self::AlmostSwapped),
            RuleIdentity::BadStringEscape => Self::basic(Self::BadStringEscape),
            RuleIdentity::CompareNan => Self::basic(Self::CompareNan),
            RuleIdentity::ConstantCondition => Self::basic(Self::ConstantCondition),
            RuleIdentity::ConstantTableComparison => Self::basic(Self::ConstantTableComparison),
            RuleIdentity::LengthAsCondition => Self::basic(Self::LengthAsCondition),
            RuleIdentity::MismatchedArgumentCount => Self::basic(Self::MismatchedArgumentCount),
            RuleIdentity::MustUse => Self::basic(Self::MustUse),
            RuleIdentity::ZeroStepLoop => Self::basic(Self::ZeroStepLoop),
            RuleIdentity::DivideByZero => Self::basic(Self::DivideByZero),
            RuleIdentity::SelfAssignment => Self::basic(Self::SelfAssignment),
            RuleIdentity::EmptyIf => Self::basic(Self::EmptyIf),
            RuleIdentity::EmptyLoop => Self::basic(Self::EmptyLoop),
            RuleIdentity::IfSameThenElse => Self::basic(Self::IfSameThenElse),
            RuleIdentity::IgnoredPcallResult => Self::basic(Self::IgnoredPcallResult),
            RuleIdentity::MixedTable => Self::basic(Self::MixedTable),
            RuleIdentity::UnscopedVariables => Self::basic(Self::UnscopedVariables),

            RuleIdentity::RobloxIncorrectColor3NewBounds => {
                Self::basic(Self::RobloxIncorrectColor3NewBounds)
            }

            RuleIdentity::RobloxManualFromscaleOrFromoffset => {
                Self::basic(Self::RobloxManualFromscaleOrFromoffset)
            }

            RuleIdentity::RobloxPreferGetPlayers => Self::basic(Self::RobloxPreferGetPlayers),
            RuleIdentity::RobloxSuspiciousUdim2New => Self::basic(Self::RobloxSuspiciousUdim2New),
            RuleIdentity::ImplicitAnyLocal => Self::basic(Self::ImplicitAnyLocal),
            RuleIdentity::ImplicitAnyParameter => Self::basic(Self::ImplicitAnyParameter),
            RuleIdentity::AndOrConditional => Self::basic(Self::AndOrConditional),
            RuleIdentity::CollapsibleIf => Self::basic(Self::CollapsibleIf),
            RuleIdentity::ElseAfterReturn => Self::basic(Self::ElseAfterReturn),
            RuleIdentity::IfExpressionAssignment => Self::basic(Self::IfExpressionAssignment),
            RuleIdentity::NegatedCondition => Self::basic(Self::NegatedCondition),
            RuleIdentity::NonConstRequire => Self::basic(Self::NonConstRequire),
            RuleIdentity::ParenthesizedConditions => Self::basic(Self::ParenthesizedConditions),
            RuleIdentity::GlobalUsage => Self::basic(Self::GlobalUsage),
            RuleIdentity::TypeCheckInsideCall => Self::basic(Self::TypeCheckInsideCall),
            RuleIdentity::LoopInvariantCall => Self::basic(Self::LoopInvariantCall),
            RuleIdentity::ManualTableClone => Self::basic(Self::ManualTableClone),
            RuleIdentity::StringConcatInLoop => Self::basic(Self::StringConcatInLoop),
            RuleIdentity::UnusedVariable => Self::UnusedVariable(UnusedBindings::default()),
            RuleIdentity::PreferConst => Self::PreferConst(ImmutableBindings::default()),
            RuleIdentity::Deprecated => Self::Deprecated(DeprecatedSymbols::default()),

            RuleIdentity::RestrictedGlobals => {
                Self::RestrictedGlobals(RestrictedGlobals::default())
            }

            RuleIdentity::RestrictedModulePaths => {
                Self::RestrictedModulePaths(RestrictedImports::default())
            }

            RuleIdentity::UnknownGlobal => Self::basic(Self::UnknownGlobal),
            RuleIdentity::DeprecatedGlobal => Self::basic(Self::DeprecatedGlobal),
            RuleIdentity::GlobalUsedAsLocal => Self::basic(Self::GlobalUsedAsLocal),
            RuleIdentity::LocalShadow => Self::basic(Self::LocalShadow),
            RuleIdentity::SameLineStatement => Self::basic(Self::SameLineStatement),
            RuleIdentity::MultiLineStatement => Self::basic(Self::MultiLineStatement),
            RuleIdentity::LocalUnused => Self::basic(Self::LocalUnused),
            RuleIdentity::FunctionUnused => Self::basic(Self::FunctionUnused),
            RuleIdentity::ImportUnused => Self::basic(Self::ImportUnused),
            RuleIdentity::BuiltinGlobalWrite => Self::basic(Self::BuiltinGlobalWrite),
            RuleIdentity::PlaceholderRead => Self::basic(Self::PlaceholderRead),
            RuleIdentity::UnreachableCode => Self::basic(Self::UnreachableCode),
            RuleIdentity::UnknownType => Self::basic(Self::UnknownType),
            RuleIdentity::ForRange => Self::basic(Self::ForRange),
            RuleIdentity::UnbalancedAssignment => Self::basic(Self::UnbalancedAssignment),
            RuleIdentity::ImplicitReturn => Self::basic(Self::ImplicitReturn),
            RuleIdentity::DuplicateLocal => Self::basic(Self::DuplicateLocal),
            RuleIdentity::FormatString => Self::basic(Self::FormatString),
            RuleIdentity::TableLiteral => Self::basic(Self::TableLiteral),
            RuleIdentity::UninitializedLocal => Self::basic(Self::UninitializedLocal),
            RuleIdentity::DuplicateFunction => Self::basic(Self::DuplicateFunction),
            RuleIdentity::DeprecatedApi => Self::basic(Self::DeprecatedApi),
            RuleIdentity::TableOperations => Self::basic(Self::TableOperations),
            RuleIdentity::DuplicateCondition => Self::basic(Self::DuplicateCondition),
            RuleIdentity::MisleadingAndOr => Self::basic(Self::MisleadingAndOr),
            RuleIdentity::CommentDirective => Self::basic(Self::CommentDirective),
            RuleIdentity::IntegerParsing => Self::basic(Self::IntegerParsing),
            RuleIdentity::ComparisonPrecedence => Self::basic(Self::ComparisonPrecedence),
            RuleIdentity::RedundantNativeAttribute => Self::basic(Self::RedundantNativeAttribute),
        }
    }

    /// Returns the only identity accepting these parameters.
    #[must_use]
    pub const fn identity(&self) -> RuleIdentity {
        match self {
            Self::AlmostSwapped(..) => RuleIdentity::AlmostSwapped,
            Self::BadStringEscape(..) => RuleIdentity::BadStringEscape,
            Self::CompareNan(..) => RuleIdentity::CompareNan,
            Self::ConstantCondition(..) => RuleIdentity::ConstantCondition,
            Self::ConstantTableComparison(..) => RuleIdentity::ConstantTableComparison,
            Self::LengthAsCondition(..) => RuleIdentity::LengthAsCondition,
            Self::MismatchedArgumentCount(..) => RuleIdentity::MismatchedArgumentCount,
            Self::MustUse(..) => RuleIdentity::MustUse,
            Self::ZeroStepLoop(..) => RuleIdentity::ZeroStepLoop,
            Self::DivideByZero(..) => RuleIdentity::DivideByZero,
            Self::SelfAssignment(..) => RuleIdentity::SelfAssignment,
            Self::EmptyIf(..) => RuleIdentity::EmptyIf,
            Self::EmptyLoop(..) => RuleIdentity::EmptyLoop,
            Self::IfSameThenElse(..) => RuleIdentity::IfSameThenElse,
            Self::IgnoredPcallResult(..) => RuleIdentity::IgnoredPcallResult,
            Self::MixedTable(..) => RuleIdentity::MixedTable,
            Self::UnscopedVariables(..) => RuleIdentity::UnscopedVariables,

            Self::RobloxIncorrectColor3NewBounds(..) => {
                RuleIdentity::RobloxIncorrectColor3NewBounds
            }

            Self::RobloxManualFromscaleOrFromoffset(..) => {
                RuleIdentity::RobloxManualFromscaleOrFromoffset
            }

            Self::RobloxPreferGetPlayers(..) => RuleIdentity::RobloxPreferGetPlayers,
            Self::RobloxSuspiciousUdim2New(..) => RuleIdentity::RobloxSuspiciousUdim2New,
            Self::ImplicitAnyLocal(..) => RuleIdentity::ImplicitAnyLocal,
            Self::ImplicitAnyParameter(..) => RuleIdentity::ImplicitAnyParameter,
            Self::AndOrConditional(..) => RuleIdentity::AndOrConditional,
            Self::CollapsibleIf(..) => RuleIdentity::CollapsibleIf,
            Self::ElseAfterReturn(..) => RuleIdentity::ElseAfterReturn,
            Self::IfExpressionAssignment(..) => RuleIdentity::IfExpressionAssignment,
            Self::NegatedCondition(..) => RuleIdentity::NegatedCondition,
            Self::NonConstRequire(..) => RuleIdentity::NonConstRequire,
            Self::ParenthesizedConditions(..) => RuleIdentity::ParenthesizedConditions,
            Self::GlobalUsage(..) => RuleIdentity::GlobalUsage,
            Self::TypeCheckInsideCall(..) => RuleIdentity::TypeCheckInsideCall,
            Self::LoopInvariantCall(..) => RuleIdentity::LoopInvariantCall,
            Self::ManualTableClone(..) => RuleIdentity::ManualTableClone,
            Self::StringConcatInLoop(..) => RuleIdentity::StringConcatInLoop,
            Self::UnusedVariable(..) => RuleIdentity::UnusedVariable,
            Self::PreferConst(..) => RuleIdentity::PreferConst,
            Self::Deprecated(..) => RuleIdentity::Deprecated,
            Self::RestrictedGlobals(..) => RuleIdentity::RestrictedGlobals,
            Self::RestrictedModulePaths(..) => RuleIdentity::RestrictedModulePaths,
            Self::UnknownGlobal(..) => RuleIdentity::UnknownGlobal,
            Self::DeprecatedGlobal(..) => RuleIdentity::DeprecatedGlobal,
            Self::GlobalUsedAsLocal(..) => RuleIdentity::GlobalUsedAsLocal,
            Self::LocalShadow(..) => RuleIdentity::LocalShadow,
            Self::SameLineStatement(..) => RuleIdentity::SameLineStatement,
            Self::MultiLineStatement(..) => RuleIdentity::MultiLineStatement,
            Self::LocalUnused(..) => RuleIdentity::LocalUnused,
            Self::FunctionUnused(..) => RuleIdentity::FunctionUnused,
            Self::ImportUnused(..) => RuleIdentity::ImportUnused,
            Self::BuiltinGlobalWrite(..) => RuleIdentity::BuiltinGlobalWrite,
            Self::PlaceholderRead(..) => RuleIdentity::PlaceholderRead,
            Self::UnreachableCode(..) => RuleIdentity::UnreachableCode,
            Self::UnknownType(..) => RuleIdentity::UnknownType,
            Self::ForRange(..) => RuleIdentity::ForRange,
            Self::UnbalancedAssignment(..) => RuleIdentity::UnbalancedAssignment,
            Self::ImplicitReturn(..) => RuleIdentity::ImplicitReturn,
            Self::DuplicateLocal(..) => RuleIdentity::DuplicateLocal,
            Self::FormatString(..) => RuleIdentity::FormatString,
            Self::TableLiteral(..) => RuleIdentity::TableLiteral,
            Self::UninitializedLocal(..) => RuleIdentity::UninitializedLocal,
            Self::DuplicateFunction(..) => RuleIdentity::DuplicateFunction,
            Self::DeprecatedApi(..) => RuleIdentity::DeprecatedApi,
            Self::TableOperations(..) => RuleIdentity::TableOperations,
            Self::DuplicateCondition(..) => RuleIdentity::DuplicateCondition,
            Self::MisleadingAndOr(..) => RuleIdentity::MisleadingAndOr,
            Self::CommentDirective(..) => RuleIdentity::CommentDirective,
            Self::IntegerParsing(..) => RuleIdentity::IntegerParsing,
            Self::ComparisonPrecedence(..) => RuleIdentity::ComparisonPrecedence,
            Self::RedundantNativeAttribute(..) => RuleIdentity::RedundantNativeAttribute,
        }
    }

    /// Validates all rule-specific parameters before evaluation.
    ///
    /// # Errors
    /// Rejects malformed names, paths, reasons, replacements, and regular expressions.
    pub fn validate(&self) -> io::Result<()> {
        match self {
            Self::UnusedVariable(settings) => {
                Regex::new(&settings.ignore_pattern)
                    .map_err(|error| invalid(format!("unused_variable.ignore_pattern: {error}")))?;
            }

            Self::Deprecated(settings) => validate_map(&settings.paths, true)?,

            Self::RestrictedGlobals(settings) => {
                validate_map(&settings.names, false)?;

                for name in settings.names.keys() {
                    if !identifier(name) {
                        return Err(invalid(format!("invalid restricted global name {name}")));
                    }
                }
            }

            Self::RestrictedModulePaths(settings) => validate_map(&settings.paths, false)?,
            _ => {}
        }

        Ok(())
    }

    fn basic(constructor: fn(BasicSettings) -> Self) -> Self {
        constructor(BasicSettings::default())
    }
}

/// One rule's effective enablement, severity, and typed parameters.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuleConfiguration {
    /// Whether the caller selected this rule.
    pub enabled: bool,

    /// Effective reporting severity, independent of enablement.
    pub severity: Severity,

    /// Identity-specific parameters.
    pub settings: RuleSettings,
}

impl RuleConfiguration {
    /// Constructs source-verified enablement and warning severity defaults.
    #[must_use]
    pub fn new(rule: RuleIdentity) -> Self {
        let enabled = !matches!(
            rule,
            RuleIdentity::ImplicitAnyLocal
                | RuleIdentity::ImplicitAnyParameter
                | RuleIdentity::AndOrConditional
                | RuleIdentity::CollapsibleIf
                | RuleIdentity::ElseAfterReturn
                | RuleIdentity::IfExpressionAssignment
                | RuleIdentity::NegatedCondition
                | RuleIdentity::NonConstRequire
                | RuleIdentity::ParenthesizedConditions
                | RuleIdentity::GlobalUsage
                | RuleIdentity::TypeCheckInsideCall
                | RuleIdentity::LoopInvariantCall
                | RuleIdentity::ManualTableClone
                | RuleIdentity::StringConcatInLoop
                | RuleIdentity::PreferConst
        );

        Self {
            enabled,
            severity: Severity::Warning,
            settings: RuleSettings::for_rule(rule),
        }
    }
}

/// Effective, caller-supplied policy; no discovery or configuration evaluation occurs here.
#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct LintSettings {
    /// Named rule policies after inheritance and invocation overrides.
    pub rules: BTreeMap<RuleIdentity, RuleConfiguration>,

    /// Environment-declared globals for unscoped assignment checks.
    pub globals: Vec<String>,

    /// Whether Roblox-specific lexical rules are applicable.
    pub roblox: bool,

    #[serde(skip)]
    #[schemars(skip)]
    /// Coverage of the caller-established language environment.
    pub environment: Availability,
}

impl Default for LintSettings {
    fn default() -> Self {
        Self {
            rules: rules()
                .iter()
                .map(|rule| (rule.identity, RuleConfiguration::new(rule.identity)))
                .collect(),
            globals: Vec::new(),
            roblox: false,
            environment: Availability::Available,
        }
    }
}

impl LintSettings {
    /// Validates identity/parameter agreement and effective rule settings.
    ///
    /// # Errors
    /// Rejects mismatched typed settings and invalid rule-specific values.
    pub fn validate(&self) -> io::Result<()> {
        for (identity, configuration) in &self.rules {
            if configuration.settings.identity() != *identity {
                return Err(invalid(format!(
                    "settings identity does not match {identity}"
                )));
            }

            configuration.settings.validate()?;
        }

        for name in &self.globals {
            if !identifier(name) {
                return Err(invalid("invalid declared global name"));
            }
        }

        Ok(())
    }

    /// Iterates selected definitions and their validated policy.
    pub fn enabled(&self) -> impl Iterator<Item = (&'static RuleDefinition, &RuleConfiguration)> {
        rules().iter().filter_map(|definition| {
            self.rules
                .get(&definition.identity)
                .filter(|configuration| configuration.enabled)
                .map(|configuration| (definition, configuration))
        })
    }

    /// Applies a named group without changing typed parameters or severities.
    pub fn apply_preset(&mut self, preset: Preset) {
        for definition in rules() {
            let configuration = self
                .rules
                .entry(definition.identity)
                .or_insert_with(|| RuleConfiguration::new(definition.identity));

            configuration.enabled = match preset {
                Preset::None => false,
                Preset::All => true,
                Preset::Correctness => RuleConfiguration::new(definition.identity).enabled,

                Preset::Recommended => {
                    RuleConfiguration::new(definition.identity).enabled
                        || matches!(
                            definition.identity,
                            RuleIdentity::PreferConst
                                | RuleIdentity::NonConstRequire
                                | RuleIdentity::TypeCheckInsideCall
                        )
                }
            };
        }
    }

    /// Applies a validated named override map atomically.
    ///
    /// # Errors
    /// Rejects unknown names, mismatched parameters, and invalid settings.
    pub fn override_named(
        &mut self,
        overrides: BTreeMap<String, RuleConfiguration>,
    ) -> io::Result<()> {
        let mut proposed = self.clone();

        for (name, configuration) in overrides {
            let identity = RuleIdentity::named(&name)
                .ok_or_else(|| invalid(format!("unknown lint rule {name}")))?;

            proposed.rules.insert(identity, configuration);
        }

        proposed.validate()?;
        *self = proposed;

        Ok(())
    }

    /// Iterates selected Luau warning identities for an explicit native request.
    pub fn upstream(&self) -> impl Iterator<Item = &UpstreamWarningIdentity> {
        self.enabled()
            .filter_map(|(definition, _)| match &definition.producer {
                RuleProducer::Upstream(identity) => Some(identity),
                RuleProducer::Instar(_) => None,
            })
    }
}

fn validate_map(values: &BTreeMap<String, String>, symbols: bool) -> io::Result<()> {
    for (name, reason) in values {
        if name.is_empty()
            || name.trim() != name
            || name.contains('\0')
            || reason.trim().is_empty()
            || reason.contains('\0')
        {
            return Err(invalid(
                "lint named settings require nonempty names and reasons",
            ));
        }

        if symbols && name.split('.').any(|component| !identifier(component)) {
            return Err(invalid(format!("invalid deprecated symbol path {name}")));
        }
    }

    Ok(())
}
