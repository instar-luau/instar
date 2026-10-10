use instar_shared::{Availability, Completion, Diagnostic, SourceSnapshot};
use serde::{Deserialize, Serialize};

use super::source::LanguageFeatures;

/// The caller-owned identity of a complete captured environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct EnvironmentIdentity(u64);

impl EnvironmentIdentity {
    /// Constructs the caller-owned identity.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the underlying caller-owned identity.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An actual captured declaration file and documentation namespace.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Definition {
    /// The exact captured source snapshot or source revision.
    pub source: SourceSnapshot,

    /// The qualified declaration documentation namespace.
    pub namespace: String,
}

/// An ordered group of captured declaration files.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct DeclarationGroup {
    /// Actual captured declaration files in priority order.
    pub definitions: Vec<Definition>,
}

/// Captured Roblox class and property capabilities.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct Capabilities {
    /// Captured class capabilities.
    pub classes: Vec<ClassCapability>,
}

/// Construction, service and property capabilities for one class.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClassCapability {
    /// The preserved binding, property or warning name.
    pub name: String,

    /// Whether the class is an available service.
    pub service: bool,

    /// Whether the class can be constructed.
    pub creatable: bool,

    /// Independent property access capabilities.
    pub properties: Vec<PropertyCapability>,
}

/// Independent read and write access for a class property.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PropertyCapability {
    /// The preserved binding, property or warning name.
    pub name: String,

    /// Whether this occurrence reads its binding.
    pub read: bool,

    /// Whether this occurrence writes its binding.
    pub write: bool,
}

/// One captured instance and its tree relationships.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InstanceRecord {
    /// The stable identity of this captured record.
    pub identity: String,

    /// The preserved binding, property or warning name.
    pub name: String,

    /// The captured instance class name.
    pub class: String,

    /// The established parent identity when present.
    pub parent: Option<String>,

    /// Captured child identities in tree order.
    pub children: Vec<String>,
}

/// The complete captured instance tree for a source environment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InstanceEnvironment {
    /// The captured instance-tree root identity.
    pub root: String,

    /// The captured source script instance identity.
    pub script: String,

    /// The complete captured instance tree.
    pub instances: Vec<InstanceRecord>,
}

/// The declarations, globals and runtime capabilities for a module.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnvironmentInput {
    /// The stable identity of this captured record.
    pub identity: EnvironmentIdentity,

    /// The immutable source language features.
    pub language: LanguageFeatures,

    /// Captured allowed global names.
    pub globals: Vec<String>,

    /// Captured declaration groups in application order.
    pub declarations: Vec<DeclarationGroup>,

    /// Captured class, construction and property capabilities.
    pub capabilities: Capabilities,

    /// The complete captured instance tree.
    pub instances: Option<InstanceEnvironment>,
}

/// Preparation diagnostics and availability for the exact environment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EnvironmentMetadata {
    /// The exact captured environment preparation input.
    pub input: EnvironmentInput,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Explicit requested prerequisite or semantic coverage.
    pub availability: Availability,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
