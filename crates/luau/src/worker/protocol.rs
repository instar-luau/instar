use std::time::Duration;

use instar_shared::{Completion, Diagnostic};
use serde::{Deserialize, Serialize};

use crate::native::{
    AnalysisIdentity, AnalysisPurpose, AnalysisRequest, ClosureIdentity, Completions,
    EnvironmentMetadata, FactsResult, InspectionResult, LintResult, ParsedMetadata,
    SemanticAvailability, SemanticCategory, UpstreamWarningSettings,
};

#[derive(Deserialize, Serialize)]
pub(crate) struct Request {
    pub identity: u64,
    pub generation: u64,
    pub remaining_budget: Option<Duration>,
    pub operation: Operation,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Operation {
    Analyze(AnalysisRequest),
    Facts(WireFactsInput),
    Lint(WireLintInput),
    Inspect(WirePositionInput),
    Complete(WirePositionInput),
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WireReference {
    pub analysis: AnalysisIdentity,
    pub frontend_revision: u64,
    pub purpose: AnalysisPurpose,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WireFactsInput {
    pub analysis: WireReference,
    pub categories: Vec<SemanticCategory>,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WireLintInput {
    pub analysis: WireReference,
    pub warnings: UpstreamWarningSettings,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct WirePositionInput {
    pub analysis: WireReference,
    pub offset: usize,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireModuleAnalysis {
    pub analysis: AnalysisIdentity,
    pub parsed: ParsedMetadata,
    pub environment: Option<EnvironmentMetadata>,
    pub diagnostics: Vec<Diagnostic>,
    pub availability: SemanticAvailability,
    pub completion: Completion,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireAnalysisResult {
    pub closure: ClosureIdentity,
    pub frontend_revision: u64,
    pub modules: Vec<WireModuleAnalysis>,
    pub completion: Completion,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Response {
    pub identity: u64,
    pub generation: u64,
    pub outcome: ResponseOutcome,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum ResponseOutcome {
    Analyzed(WireAnalysisResult),
    Facts(FactsResult),
    Linted(LintResult),
    Inspected(InspectionResult),
    Completed(Completions),
}
