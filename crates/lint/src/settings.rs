use std::{collections::BTreeMap, io};

use instar_luau::UpstreamWarningIdentity;
use instar_shared::{Availability, Severity, error::invalid};
use instar_syntax::identifier;
use regex::Regex;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    Preset, RuleDefinition, RuleIdentity, RuleProducer,
    catalog::inventory,
    rules,
    rules::{
        bindings::{DeprecatedSymbols, ImmutableBindings, RestrictedGlobals, UnusedBindings},
        calls::RestrictedImports,
    },
};

/// Parameterless rule settings.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BasicSettings {}

macro_rules! define_settings {
    ($(
        $identity:ident, $name:literal, $parameters:ty, $schema:ident,
        $summary:literal, $description:literal, $documentation:literal,
        $category:ident, $prerequisites:expr, $fixes:ident, $producer:ident $(($evaluator:path))?;
    )*) => {
        /// Typed parameters for each catalog identity.
        #[derive(Clone, Debug, Deserialize, Serialize, JsonSchema)]
        #[serde(tag = "rule", content = "parameters")]
        pub enum RuleSettings {
            $(
                #[doc = concat!("Parameters for `", stringify!($identity), "`.")]
                #[serde(rename = $name)]
                $identity($parameters),
            )*
        }

        impl RuleSettings {
            /// Constructs default typed parameters for a catalog identity.
            #[must_use]
            pub fn for_rule(rule: RuleIdentity) -> Self {
                match rule { $(RuleIdentity::$identity => Self::$identity(<$parameters>::default()),)* }
            }

            /// Returns the only identity accepting these parameters.
            #[must_use]
            pub const fn identity(&self) -> RuleIdentity {
                match self { $(Self::$identity(..) => RuleIdentity::$identity,)* }
            }
        }
    };
}

inventory!(define_settings);

impl RuleSettings {
    /// Validates all rule-specific parameters before evaluation.
    ///
    /// # Errors
    /// Rejects malformed names, paths, reasons, replacements, and regular expressions.
    pub fn validate(&self) -> io::Result<()> {
        match self {
            Self::UnusedVariable(settings) => {
                Regex::new(&settings.ignore_pattern).map_err(|error| {
                    invalid(format!(
                        "unused_variable.ignore_pattern: invalid regular expression: {error}"
                    ))
                })?;
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
