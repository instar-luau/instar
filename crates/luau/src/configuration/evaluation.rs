use instar_shared::{Completion, ContextIdentity, Diagnostic, SourceSnapshot, SourceVersion};
use serde::{Deserialize, Serialize};

use crate::native::LanguageFeatures;

/// Captured configuration evaluation context.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigurationContext {
    /// The stable identity of this captured record.
    pub identity: ContextIdentity,

    /// The defining layer directory.
    pub base: String,

    /// Whether the captured source is executable Luau rather than JSON.
    pub executable: bool,
}

/// Explicit configuration VM and extraction resource bounds.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigurationLimits {
    /// The bound on configuration VM allocations, not total host allocations.
    pub memory_bytes: u64,

    /// The maximum extracted table depth.
    pub extraction_depth: u32,

    /// The maximum total extracted table entry count.
    pub extraction_entries: u32,
}

/// Captured configuration source, language and evaluation limits.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ConfigurationInput {
    /// The exact captured source snapshot or source revision.
    pub source: SourceSnapshot,

    /// The immutable source language features.
    pub language: LanguageFeatures,

    /// The defining layer and evaluation context.
    pub context: ConfigurationContext,

    /// Explicit VM allocation and extraction bounds.
    pub limits: ConfigurationLimits,
}

/// An owned finite numeric or string table key.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ConfigurationKey {
    /// An owned string scalar.
    String(String),

    /// A finite numeric scalar.
    Number(f64),
}

/// One owned table key and value.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConfigurationEntry {
    /// The owned string or finite numeric table key.
    pub key: ConfigurationKey,

    /// The owned extracted table value.
    pub value: ConfigurationValue,
}

/// An owned configuration scalar or table.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ConfigurationValue {
    /// An owned string scalar.
    String(String),

    /// A finite numeric scalar.
    Number(f64),

    /// A boolean scalar.
    Boolean(bool),

    /// An owned table with deterministic entry order.
    Table(Vec<ConfigurationEntry>),
}

/// Values and diagnostics from one isolated evaluation.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ConfigurationResult {
    /// The exact captured source snapshot or source revision.
    pub source: SourceVersion,

    /// Extracted values only after complete successful evaluation.
    pub values: Option<ConfigurationValue>,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
