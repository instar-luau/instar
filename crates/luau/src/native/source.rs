use std::collections::BTreeMap;

use instar_shared::{
    Completion, ContextIdentity, Diagnostic, Location, SourceIdentity, SourceSnapshot,
};

use serde::{Deserialize, Serialize};

use crate::flags::FlagValue;

/// A source identity within a project context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct ModuleIdentity {
    /// The owning project context identity.
    pub context: ContextIdentity,

    /// The stable source identity within that context.
    pub source: SourceIdentity,
}

/// Source language flags and declaration syntax requirements.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct LanguageFeatures {
    /// Typed normalized language flag selections.
    pub flags: BTreeMap<String, FlagValue>,

    /// Whether declaration syntax is accepted.
    pub declaration_syntax: bool,
}

/// A captured source with its module identity and language features.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceInput {
    /// The context-specific source module identity.
    pub module: ModuleIdentity,

    /// The exact captured source snapshot or source revision.
    pub source: SourceSnapshot,

    /// The immutable source language features.
    pub language: LanguageFeatures,
}

/// The caller-owned identity of a source import site.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct ImportIdentity(
    /// The caller-owned import site discriminator.
    pub u64,
);

/// The captured instance root used by an import path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum InstanceRoot {
    /// The source script instance.
    Script,

    /// The root data-model instance.
    Game,

    /// The workspace instance.
    Workspace,
}

/// One operation in a captured instance import path.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum InstanceStep {
    /// The parent instance.
    Parent,

    /// A named instance field.
    Field(String),

    /// A named child lookup.
    Child {
        /// The preserved binding, property or warning name.
        name: String,
        /// Whether the child lookup searches descendants recursively.
        recursive: bool,
    },

    /// A named service lookup.
    Service(String),
}

/// A static instance root and ordered traversal steps.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InstancePath {
    /// The static instance root used by this traversal.
    pub root: InstanceRoot,

    /// Instance traversal steps in execution order.
    pub steps: Vec<InstanceStep>,
}

/// The host-extracted static, instance or nonconstant import request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ImportRequest {
    /// A captured static string request.
    Static(String),

    /// A captured instance path request.
    Instance(InstancePath),

    /// A request that cannot be statically established.
    Nonconstant,
}

/// A revision-anchored loader call and argument.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ImportSite {
    /// The stable identity of this captured record.
    pub identity: ImportIdentity,

    /// The exact loader call range.
    pub call: Location,

    /// The exact captured argument range.
    pub argument: Location,

    /// The host-extracted import request.
    pub request: ImportRequest,
}

/// A captured native source directive.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SourceDirective {
    /// The exact captured source range.
    pub location: Location,

    /// The captured directive text.
    pub text: String,
}

/// Parsing outcomes for the exact captured source input.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ParsedMetadata {
    /// The exact captured source and language input.
    pub input: SourceInput,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Native source directives in source order.
    pub directives: Vec<SourceDirective>,

    /// Host-extracted import sites in source order.
    pub imports: Vec<ImportSite>,

    /// Whether native parsing recovered from source errors.
    pub recovered_syntax: bool,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
