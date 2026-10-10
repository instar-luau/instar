use instar_shared::{Completion, Diagnostic, Location};
use serde::{Deserialize, Serialize};

use super::{
    environment::{EnvironmentInput, EnvironmentMetadata},
    revision::AnalysisReference,
    semantics::{AnalysisIdentity, SemanticAvailability, SemanticCategory},
    source::{ImportSite, ModuleIdentity, ParsedMetadata, SourceInput},
};

use crate::flags::LanguageProfile;

/// The caller-owned identity of one supplied module closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct ClosureIdentity(u64);

impl ClosureIdentity {
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

/// The source or editor cache purpose of an analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum AnalysisPurpose {
    /// Source checking with explicitly requested query caches only.
    Source,

    /// Editor analysis retaining query caches.
    Editor,
}

/// The requested type-inference checking mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum InferenceMode {
    /// Strict type checking.
    Strict,

    /// Nonstrict type checking.
    Nonstrict,
}

/// Explicit inference and retained-category requirements.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InferenceSettings {
    /// Whether native inference is explicitly required.
    pub required: bool,

    /// The selected language checking mode.
    pub mode: InferenceMode,

    /// Semantic categories whose query caches must be retained.
    pub retained: Vec<SemanticCategory>,
}

/// Diagnostic reporting policy independent of inference.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ReportingSettings {
    /// Whether ordinary source type findings are reported.
    pub ordinary_type_diagnostics: bool,
}

/// The host-established outcome of an import request.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum ImportOutcome {
    /// A captured target module.
    Module(ModuleIdentity),

    /// A static request without an established target.
    Unresolved,

    /// A request that cannot be statically established.
    Nonconstant,
}

/// An exact captured import site and host-established target.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ResolvedImport {
    /// The exact captured import call location.
    pub site: ImportSite,

    /// The host-established resolution outcome.
    pub outcome: ImportOutcome,
}

/// A captured source-site edge in a dependency cycle.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CycleEdge {
    /// The source module of this cycle edge.
    pub from: ModuleIdentity,

    /// The target module of this cycle edge.
    pub to: ModuleIdentity,

    /// The exact captured import call location.
    pub site: Location,
}

/// An ordered closed path through captured imports.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct CyclePath {
    /// Source-site edges in closed path order.
    pub edges: Vec<CycleEdge>,
}

/// All immutable source, environment and checking inputs for one module.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct ModuleInput {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisIdentity,

    /// The exact captured source snapshot or source revision.
    pub source: SourceInput,

    /// The complete captured environment or its preparation outcome.
    pub environment: Option<EnvironmentInput>,

    /// Explicit inference and retention requirements.
    pub inference: InferenceSettings,

    /// Ordinary diagnostic reporting policy.
    pub reporting: ReportingSettings,

    /// Host-extracted import sites with captured resolution outcomes.
    pub imports: Vec<ResolvedImport>,
}

/// An explicit captured closure with one immutable language profile.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AnalysisRequest {
    /// The identity of the supplied module closure.
    pub closure: ClosureIdentity,

    /// The immutable normalized language and resource profile.
    pub profile: LanguageProfile,

    /// The source or editor cache purpose.
    pub purpose: AnalysisPurpose,

    /// The supplied root module identities.
    pub roots: Vec<ModuleIdentity>,

    /// Supplied modules in deterministic identity order.
    pub modules: Vec<ModuleInput>,

    /// Host-established dependency cycle paths.
    pub cycles: Vec<CyclePath>,
}

/// Parsing, environment and inference outcomes for one supplied module.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModuleAnalysis {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisIdentity,

    /// A query reference only when parsing state was established.
    pub reference: Option<AnalysisReference>,

    /// Owned metadata from actual native parsing.
    pub parsed: ParsedMetadata,

    /// The complete captured environment or its preparation outcome.
    pub environment: Option<EnvironmentMetadata>,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Explicit requested prerequisite or semantic coverage.
    pub availability: SemanticAvailability,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}

/// The terminal outcome for the exact supplied closure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct AnalysisResult {
    /// The identity of the supplied module closure.
    pub closure: ClosureIdentity,

    /// Supplied modules in deterministic identity order.
    pub modules: Vec<ModuleAnalysis>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
