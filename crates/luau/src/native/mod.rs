//! Captured analysis inputs, guarded references and detached native language records.

mod analysis;
mod client;
mod completion;
mod environment;
mod linting;
mod revision;
mod semantics;
mod source;

pub use analysis::{
    AnalysisPurpose, AnalysisRequest, AnalysisResult, ClosureIdentity, CycleEdge, CyclePath,
    ImportOutcome, InferenceMode, InferenceSettings, ModuleAnalysis, ModuleInput,
    ReportingSettings, ResolvedImport,
};

pub use client::Native;

pub use completion::{
    CompletionCandidate, CompletionContext, CompletionKind, Completions, Notation, Parentheses,
};

pub use environment::{
    Capabilities, ClassCapability, DeclarationGroup, Definition, EnvironmentIdentity,
    EnvironmentInput, EnvironmentMetadata, InstanceEnvironment, InstanceRecord, PropertyCapability,
};

pub use linting::{
    LanguageMode, LintInput, LintResult, UpstreamFinding, UpstreamWarningIdentity,
    UpstreamWarningSettings, WarningAvailability,
};

pub use revision::AnalysisReference;

pub use semantics::{
    AnalysisIdentity, ArgumentRelation, BindingFact, BindingKind, CallFact, CallableSignature,
    ConstantValue, ControlFlow, Determinism, EffectFact, ExpressionFact, FactsInput, FactsResult,
    FunctionFact, InheritanceFact, InspectionResult, LanguageIdentity, NominalFact,
    OccurrenceAccess, OccurrenceFact, OccurrenceRole, PositionFacts, PositionInput, ScopeFact,
    SemanticAvailability, SemanticCategory, SemanticFacts, SemanticIdentity, SemanticKind,
    SignatureParameter, TypeFact,
};

pub use source::{
    ImportIdentity, ImportRequest, ImportSite, InstancePath, InstanceRoot, InstanceStep,
    LanguageFeatures, ModuleIdentity, ParsedMetadata, SourceDirective, SourceInput,
};
