use std::collections::BTreeMap;

use instar_shared::{Availability, Completion, Diagnostic, Knowledge, Location};
use serde::{Deserialize, Serialize};

use super::{
    environment::EnvironmentIdentity, revision::AnalysisReference, source::ModuleIdentity,
};

/// The logical module analysis revision for exact captured inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct AnalysisIdentity {
    /// The context-specific source module identity.
    pub module: ModuleIdentity,

    /// The logical or captured source revision.
    pub revision: u64,
}

/// A separately requested and reported semantic category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub enum SemanticCategory {
    /// Binding declarations and relationships.
    Bindings,

    /// Source reads, writes and declarations.
    Occurrences,

    /// Inferred type evidence.
    Types,

    /// Established scalar constants.
    Constants,

    /// Established dependency evidence.
    Dependencies,

    /// Callable targets and argument relationships.
    Calls,

    /// Established execution effect evidence.
    Effects,

    /// Lexical scope relationships.
    Scopes,

    /// Function declarations and signatures.
    Functions,

    /// Nominal declarations.
    Nominals,

    /// Nominal inheritance relationships.
    Inheritance,
}

/// Coverage for exactly the requested semantic categories.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct SemanticAvailability {
    /// Coverage or selection of semantic categories.
    pub categories: BTreeMap<SemanticCategory, Availability>,
}

/// The role distinguishing otherwise coincident semantic identities.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub enum SemanticKind {
    /// A lexical or global binding.
    Binding,

    /// A function parameter.
    Parameter,

    /// A member property.
    Property,

    /// An implicit method receiver.
    Receiver,

    /// A type declaration or reference.
    Type,

    /// A function declaration or function occurrence.
    Function,

    /// A nominal class declaration.
    Class,

    /// A captured target module.
    Module,

    /// A lexical scope.
    Scope,
}

/// An owned source or environment identity without native pointers.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum SemanticIdentity {
    /// A source-anchored semantic identity.
    Source {
        /// The exact captured source range.
        location: Location,
        /// The declaration or semantic role.
        kind: SemanticKind,
        /// A deterministic discriminator for coincident source identities.
        ordinal: u32,
    },

    /// An environment-anchored semantic identity.
    Environment {
        /// The complete captured environment or its preparation outcome.
        environment: EnvironmentIdentity,
        /// The qualified declaration documentation namespace.
        namespace: String,
        /// The preserved binding, property or warning name.
        name: String,
        /// The declaration or semantic role.
        kind: SemanticKind,
    },
}

/// The declaration form of a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum BindingKind {
    /// A local binding.
    Local,

    /// A function parameter.
    Parameter,

    /// A loop binding.
    Loop,

    /// A function declaration or function occurrence.
    Function,

    /// A global binding.
    Global,

    /// A member property.
    Property,

    /// A type declaration or reference.
    Type,

    /// An implicit method receiver.
    Receiver,
}

/// The syntactic role of an occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum OccurrenceRole {
    /// A variable occurrence.
    Variable,

    /// A function parameter.
    Parameter,

    /// A function declaration or function occurrence.
    Function,

    /// A member property.
    Property,

    /// A method occurrence.
    Method,

    /// A type declaration or reference.
    Type,
}

/// An established standard-language or Roblox binding identity.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum LanguageIdentity {
    /// An established standard Luau identity.
    Builtin(String),

    /// An established Roblox identity.
    Roblox(String),
}

/// An established inferred type and its declarations.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct TypeFact {
    /// The owned native type display text.
    pub display: String,

    /// Whether the established type is any.
    pub is_any: bool,

    /// Established declaration targets.
    pub declarations: Vec<SemanticIdentity>,
}

/// A detached binding, scope and shadow relationship.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct BindingFact {
    /// The stable identity of this captured record.
    pub identity: SemanticIdentity,

    /// The established declaration location or known absence.
    pub declaration: Knowledge<Option<Location>>,

    /// The preserved binding, property or warning name.
    pub name: String,

    /// The declaration or semantic role.
    pub kind: BindingKind,

    /// Whether the binding has an explicit type annotation.
    pub annotation: bool,

    /// The owning lexical scope identity.
    pub scope: SemanticIdentity,

    /// The established shadowed binding or known absence.
    pub shadows: Knowledge<Option<SemanticIdentity>>,

    /// Established inferred type evidence or its unavailable reason.
    pub type_information: Knowledge<TypeFact>,

    /// The qualified documentation identity when established.
    pub documentation: Option<String>,

    /// Whether the native language marks this record deprecated.
    pub deprecated: bool,

    /// A positively established language binding identity.
    pub language_identity: Option<LanguageIdentity>,
}

/// Established read and write access at an occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum OccurrenceAccess {
    /// Neither read nor write access is established.
    None,

    /// The occurrence reads the binding.
    Read,

    /// The occurrence writes the binding.
    Write,

    /// The occurrence both reads and writes the binding.
    ReadWrite,
}

impl OccurrenceAccess {
    /// Whether this access reads the binding.
    #[must_use]
    pub fn reads(self) -> bool {
        matches!(self, Self::Read | Self::ReadWrite)
    }

    /// Whether this access writes the binding.
    #[must_use]
    pub fn writes(self) -> bool {
        matches!(self, Self::Write | Self::ReadWrite)
    }
}

/// A revision-anchored read, write or declaration occurrence.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct OccurrenceFact {
    /// The exact captured source range.
    pub location: Location,

    /// Established semantic targets or their unavailable reason.
    pub targets: Knowledge<Vec<SemanticIdentity>>,

    /// Whether this occurrence declares its binding.
    pub declaration: bool,

    /// Established read and write access.
    pub access: OccurrenceAccess,

    /// The syntactic occurrence role.
    pub role: OccurrenceRole,

    /// Whether the native language marks this record deprecated.
    pub deprecated: bool,
}

/// An established scalar constant.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub enum ConstantValue {
    /// The nil constant.
    Nil,

    /// A boolean scalar.
    Boolean(bool),

    /// A finite numeric scalar.
    Number(f64),

    /// An owned string scalar.
    String(String),
}

/// Whether deterministic execution has been established.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Determinism {
    /// Execution is established to be deterministic.
    Deterministic,

    /// Deterministic execution has not been established.
    Unproven,
}

/// Whether moving an operation preserves its control-flow behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ControlFlow {
    /// Control-flow preservation has been established.
    Preserved,

    /// Control-flow preservation has not been established.
    Unproven,
}

/// Established dependencies and conservative execution effects.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct EffectFact {
    /// Established identities read by the operation.
    pub reads: Vec<SemanticIdentity>,

    /// Established identities written by the operation.
    pub writes: Vec<SemanticIdentity>,

    /// Whether established execution may throw.
    pub may_throw: bool,

    /// Whether established execution may yield.
    pub may_yield: bool,

    /// Whether established execution may allocate.
    pub may_allocate: bool,

    /// Established determinism or absence of its proof.
    pub determinism: Determinism,

    /// Established control-flow preservation or absence of its proof.
    pub control_flow: ControlFlow,
}

/// Detached type, constant, dependency and effect evidence.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ExpressionFact {
    /// The exact captured source range.
    pub location: Location,

    /// Established inferred type evidence or its unavailable reason.
    pub type_information: Knowledge<TypeFact>,

    /// An established constant, established nonconstant value, or unavailable reason.
    pub constant: Knowledge<Option<ConstantValue>>,

    /// Established dependencies or their unavailable reason.
    pub dependencies: Knowledge<Vec<SemanticIdentity>>,

    /// Established execution effects or their unavailable reason.
    pub effects: Knowledge<EffectFact>,
}

/// A parameter label range and optional source declaration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct SignatureParameter {
    /// The UTF-8 byte range within the signature label.
    pub label_range: [usize; 2],

    /// The established declaration location or known absence.
    pub declaration: Option<Location>,
}

/// An owned callable signature and argument constraints.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CallableSignature {
    /// The owned callable signature display text.
    pub label: String,

    /// Parameters in signature order.
    pub parameters: Vec<SignatureParameter>,

    /// Whether additional arguments are accepted.
    pub variadic: bool,

    /// The established minimum argument count or unavailable reason.
    pub minimum_arguments: Knowledge<usize>,

    /// The qualified documentation identity when established.
    pub documentation: Option<String>,
}

/// A call argument and its established signature parameter.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ArgumentRelation {
    /// The exact captured argument range.
    pub argument: Location,

    /// The established parameter index or known absence.
    pub parameter: Knowledge<Option<usize>>,
}

/// A detached call relationship with argument and effect evidence.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CallFact {
    /// The exact captured source range.
    pub location: Location,

    /// The exact callee expression range within this call.
    pub callee: Location,

    /// The enclosing function or module identity.
    pub caller: SemanticIdentity,

    /// Established semantic targets or their unavailable reason.
    pub targets: Knowledge<Vec<SemanticIdentity>>,

    /// Established callable signatures or their unavailable reason.
    pub signatures: Knowledge<Vec<CallableSignature>>,

    /// Call arguments in source order.
    pub arguments: Vec<ArgumentRelation>,

    /// Established execution effects or their unavailable reason.
    pub effects: Knowledge<EffectFact>,
}

/// An established lexical scope and its parent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ScopeFact {
    /// The stable identity of this captured record.
    pub identity: SemanticIdentity,

    /// The exact captured source range.
    pub location: Location,

    /// The established parent identity when present.
    pub parent: Option<SemanticIdentity>,
}

/// A detached function declaration, body and signatures.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FunctionFact {
    /// The stable identity of this captured record.
    pub identity: SemanticIdentity,

    /// The exact captured source range.
    pub location: Location,

    /// The exact declaration name selection range.
    pub selection: Location,

    /// The exact captured function body range.
    pub body: Location,

    /// The owning lexical scope identity.
    pub scope: SemanticIdentity,

    /// Established callable signatures or their unavailable reason.
    pub signatures: Knowledge<Vec<CallableSignature>>,
}

/// A detached nominal declaration.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct NominalFact {
    /// The stable identity of this captured record.
    pub identity: SemanticIdentity,

    /// The preserved binding, property or warning name.
    pub name: String,

    /// The established declaration location or known absence.
    pub declaration: Location,

    /// The exact declaration name selection range.
    pub selection: Location,

    /// The qualified documentation identity when established.
    pub documentation: Option<String>,

    /// Whether the native language marks this record deprecated.
    pub deprecated: bool,
}

/// An established child-to-parent nominal relationship.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InheritanceFact {
    /// The established child identity.
    pub child: SemanticIdentity,

    /// The established parent identity when present.
    pub parent: SemanticIdentity,
}

/// Owned semantic records in stable source order.
#[derive(Clone, Debug, Default, PartialEq, Deserialize, Serialize)]
pub struct SemanticFacts {
    /// Detached binding records.
    pub bindings: Vec<BindingFact>,

    /// Detached occurrence records.
    pub occurrences: Vec<OccurrenceFact>,

    /// Detached expression evidence.
    pub expressions: Vec<ExpressionFact>,

    /// Detached call relationships.
    pub calls: Vec<CallFact>,

    /// Detached lexical scopes.
    pub scopes: Vec<ScopeFact>,

    /// Detached function declarations.
    pub functions: Vec<FunctionFact>,

    /// Detached nominal declarations.
    pub nominals: Vec<NominalFact>,

    /// Established nominal inheritance relationships.
    pub inheritance: Vec<InheritanceFact>,
}

/// Requested categories from one guarded established analysis.
#[derive(Clone, Debug)]
pub struct FactsInput {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisReference,

    /// Coverage or selection of semantic categories.
    pub categories: Vec<SemanticCategory>,
}

/// Detached semantic records with explicit requested coverage.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct FactsResult {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisIdentity,

    /// Owned semantic records.
    pub facts: SemanticFacts,

    /// Explicit requested prerequisite or semantic coverage.
    pub availability: SemanticAvailability,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}

/// A guarded analysis and UTF-8 byte cursor.
#[derive(Clone, Debug)]
pub struct PositionInput {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisReference,

    /// A UTF-8 byte cursor in the referenced source.
    pub offset: usize,
}

/// Information established at one captured source position.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct PositionFacts {
    /// The exact captured source range.
    pub location: Location,

    /// Established semantic targets or their unavailable reason.
    pub targets: Knowledge<Vec<SemanticIdentity>>,

    /// Established inferred type evidence or its unavailable reason.
    pub type_information: Knowledge<TypeFact>,

    /// Established declaration targets.
    pub declarations: Knowledge<Vec<Location>>,

    /// Established definition targets.
    pub definitions: Knowledge<Vec<Location>>,

    /// Established type-definition targets.
    pub type_definitions: Knowledge<Vec<Location>>,

    /// The qualified documentation identity when established.
    pub documentation: Knowledge<Option<String>>,

    /// Whether the native language marks this record deprecated.
    pub deprecated: bool,

    /// Established callable signatures or their unavailable reason.
    pub signatures: Knowledge<Vec<CallableSignature>>,

    /// The active signature index when established.
    pub active_signature: Option<usize>,

    /// The active argument index when established.
    pub active_argument: Option<usize>,

    /// Established hierarchy identities at this position.
    pub hierarchy: Knowledge<Vec<SemanticIdentity>>,
}

/// Position information and its explicit semantic coverage.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InspectionResult {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisIdentity,

    /// Position facts when a match was established.
    pub occurrence: Option<PositionFacts>,

    /// Explicit requested prerequisite or semantic coverage.
    pub availability: SemanticAvailability,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
