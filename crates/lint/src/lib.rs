//! Catalog-driven Luau lint rules over caller-supplied snapshots and evidence.

mod catalog;
mod fixes;
mod request;
mod result;
mod rules;
mod settings;

pub use catalog::{
    FixSupport, Prerequisites, Preset, RuleCategory, RuleDefinition, RuleIdentity, RuleProducer,
    RuleSchema, rules,
};

pub use fixes::{FixValidation, RuleFix, collect_fixes};
pub use request::{ResolvedDependency, RuleInput, run};
pub use result::{RuleAvailability, RuleResult};

pub use rules::{
    bindings::{DeprecatedSymbols, ImmutableBindings, RestrictedGlobals, UnusedBindings},
    calls::RestrictedImports,
};

pub use settings::{BasicSettings, LintSettings, RuleConfiguration, RuleSettings};
