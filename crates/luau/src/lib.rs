//! Captured Luau analysis, guarded semantic queries and isolated configuration evaluation.

#[expect(
    clippy::absolute_paths,
    clippy::arbitrary_source_item_ordering,
    reason = "CXX generates fully qualified paths and pointer trait implementations after factory functions"
)]
#[expect(
    unsafe_code,
    unreachable_pub,
    reason = "CXX generates public unsafe declarations inside this private native boundary"
)]
mod boundary;

mod configuration;

#[path = "boundary/conversion.rs"]
mod conversion;

pub mod flags;
mod native;
mod worker;

pub use configuration::{
    ConfigurationContext, ConfigurationEntry, ConfigurationEvaluator, ConfigurationInput,
    ConfigurationKey, ConfigurationLimits, ConfigurationResult, ConfigurationValue,
    run_configuration_worker,
};

pub use flags::{FlagValue, LanguageProfile, SolverMode};

pub use native::{
    AnalysisIdentity, AnalysisPurpose, AnalysisReference, AnalysisRequest, AnalysisResult,
    ArgumentRelation, BindingFact, BindingKind, CallFact, CallableSignature, Capabilities,
    ClassCapability, ClosureIdentity, CompletionCandidate, CompletionContext, CompletionKind,
    Completions, ConstantValue, ControlFlow, CycleEdge, CyclePath, DeclarationGroup, Definition,
    Determinism, EffectFact, EnvironmentIdentity, EnvironmentInput, EnvironmentMetadata,
    ExpressionFact, FactsInput, FactsResult, FunctionFact, ImportIdentity, ImportOutcome,
    ImportRequest, ImportSite, InferenceMode, InferenceSettings, InheritanceFact, InspectionResult,
    InstanceEnvironment, InstancePath, InstanceRecord, InstanceRoot, InstanceStep,
    LanguageFeatures, LanguageIdentity, LanguageMode, LintInput, LintResult, ModuleAnalysis,
    ModuleIdentity, ModuleInput, Native, NominalFact, Notation, OccurrenceAccess, OccurrenceFact,
    OccurrenceRole, Parentheses, ParsedMetadata, PositionFacts, PositionInput, PropertyCapability,
    ReportingSettings, ResolvedImport, ScopeFact, SemanticAvailability, SemanticCategory,
    SemanticFacts, SemanticIdentity, SemanticKind, SignatureParameter, SourceDirective,
    SourceInput, TypeFact, UpstreamFinding, UpstreamWarningIdentity, UpstreamWarningSettings,
    WarningAvailability,
};

pub use worker::run_worker;
