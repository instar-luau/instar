use std::io;

use instar_format::FormatResult;
use instar_lint::{RuleAvailability, RuleFix, RuleIdentity};
use instar_luau::{ImportOutcome, ImportSite, ModuleIdentity};

use instar_shared::{
    Availability, Completion, Diagnostic, EditIdentity, EditProposal, SourceIdentity,
    SourceOutcome, SourceVersion,
};

use serde::{Serialize, Serializer, ser::SerializeStruct};

use crate::{
    configuration::{EffectiveSettings, Operation},
    editing::{ValidatedEdit, WriteOutcome},
    graph::DependencyEdge,
    inputs::{InputVersion, ProjectSnapshot},
    queries::QueryOutput,
    selection::SelectionDecision,
};

/// Operation totals for selection, findings, and requested edits.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Counts {
    /// Directly selected targets.
    pub selected: usize,

    /// Additional dependency modules.
    pub dependencies: usize,

    /// Candidates excluded by discovery or selection policy.
    pub excluded: usize,

    /// Findings retained for reporting.
    pub findings: usize,

    /// Sources already matching the requested formatting.
    pub unchanged: usize,

    /// Sources with changed formatting or fix proposals.
    pub changed: usize,

    /// Sources with unavailable processing or unsuccessful application.
    pub unsuccessful: usize,

    /// Sources successfully written.
    pub written: usize,

    /// Fix proposals whose edits were all accepted.
    pub fixes: usize,
}

/// Captured dependency-aware checking results and reporting-policy outcome.
#[derive(Debug)]
pub struct CheckReport {
    /// Snapshot containing the source revisions used by the report.
    pub snapshot: ProjectSnapshot,

    /// Consulted input guards for freshness checks.
    pub inputs: Vec<InputVersion>,

    /// Distinct source revisions in the captured dependency graph.
    pub sources: Vec<SourceVersion>,

    /// Analyzed module contexts.
    pub modules: Vec<ModuleIdentity>,

    /// Findings attributed to their analyzed module contexts.
    pub module_findings: Vec<ModuleDiagnostic>,

    /// Resolution evidence for captured import sites.
    pub imports: Vec<ImportExplanation>,

    /// Findings retained by the requested reporting policy.
    pub findings: Vec<Diagnostic>,

    /// Environment and declaration prerequisite diagnostics.
    pub environment_diagnostics: Vec<Diagnostic>,

    /// Input selection evidence.
    pub selection: Vec<SelectionDecision>,

    /// Resolved import edges in the captured dependency graph.
    pub dependencies: Vec<DependencyEdge>,

    /// Effective settings for captured modules.
    pub settings: Vec<EffectiveSettings>,

    /// Selection and finding totals.
    pub counts: Counts,

    /// Completion state, independent of diagnostic severity policy.
    pub completion: Completion,

    /// Whether findings fail the requested reporting policy.
    pub failed: bool,
}

/// Formatter output and any associated validation, preview, and application result.
#[derive(Debug)]
pub struct FormattedSource {
    /// Formatter output with its diagnostics and producer completion.
    pub result: FormatResult,

    /// Validated edit capability when changed text was prepared successfully.
    pub proposal: Option<ValidatedEdit>,

    /// Requested textual diff preview.
    pub preview: Option<String>,

    /// Application outcome for this source.
    pub write: WriteOutcome,
}

/// Captured formatting results for the requested output mode.
#[derive(Debug)]
pub struct FormatReport {
    /// Snapshot containing the source revisions used by the report.
    pub snapshot: ProjectSnapshot,

    /// Consulted input guards for freshness checks.
    pub inputs: Vec<InputVersion>,

    /// Prepared per-source formatter results.
    pub results: Vec<FormattedSource>,

    /// Input selection evidence.
    pub selection: Vec<SelectionDecision>,

    /// Effective settings for selected sources.
    pub settings: Vec<EffectiveSettings>,

    /// Selection, formatting, and application totals.
    pub counts: Counts,

    /// Project diagnostics accompanying the formatter results.
    pub diagnostics: Vec<Diagnostic>,

    /// Completion state of preparation and requested application.
    pub completion: Completion,

    /// Whether formatting checks, preparation, or requested application failed.
    pub failed: bool,
}

/// Captured lint findings, rule coverage, and requested fix outcomes.
#[derive(Debug)]
pub struct LintReport {
    /// Snapshot associated with the final findings.
    pub snapshot: ProjectSnapshot,

    /// Consulted input guards for freshness checks.
    pub inputs: Vec<InputVersion>,

    /// Source revisions retained across initial and final linting.
    pub sources: Vec<SourceVersion>,

    /// Module contexts associated with the final findings.
    pub modules: Vec<ModuleIdentity>,

    /// Findings attributed to their analyzed module contexts.
    pub module_findings: Vec<ModuleDiagnostic>,

    /// Resolution evidence for captured import sites.
    pub imports: Vec<ImportExplanation>,

    /// Per-source producer and rule outcomes.
    pub outcomes: Vec<SourceOutcome>,

    /// Snapshots retained for resolving initial, validation, and final locations.
    pub snapshots: Vec<ProjectSnapshot>,

    /// Initial findings retained by the requested reporting policy.
    pub findings: Vec<Diagnostic>,

    /// Availability of enabled rules in each analyzed module context.
    pub availability: Vec<RuleAvailability>,

    /// Rule-produced fix candidates.
    pub fixes: Vec<RuleFix>,

    /// Combined fix proposals that completed required validation.
    pub proposals: Vec<ValidatedEdit>,

    /// Requested textual diff previews.
    pub previews: Vec<String>,

    /// Outcomes of requested source application.
    pub writes: Vec<WriteOutcome>,

    /// Findings after accepted fixes, or initial findings when none were accepted.
    pub final_findings: Vec<Diagnostic>,

    /// Input selection evidence.
    pub selection: Vec<SelectionDecision>,

    /// Effective settings associated with the final lint results.
    pub settings: Vec<EffectiveSettings>,

    /// Resolved import edges associated with the final lint results.
    pub dependencies: Vec<DependencyEdge>,

    /// Selection, finding, fix, and application totals.
    pub counts: Counts,

    /// Combined completion state of linting and requested fix processing.
    pub completion: Completion,

    /// Whether reporting policy or requested fix processing failed.
    pub failed: bool,
}

/// A query result with its captured inputs and explicit availability.
#[derive(Debug)]
pub struct QueryReport {
    /// Snapshot containing the source revisions used by the query.
    pub snapshot: ProjectSnapshot,

    /// Consulted input guards for freshness checks.
    pub inputs: Vec<InputVersion>,

    /// Detached result of the requested query.
    pub output: QueryOutput,

    /// Availability of the requested result, separate from operation completion.
    pub availability: Availability,

    /// Diagnostics retained while preparing the result.
    pub diagnostics: Vec<Diagnostic>,

    /// Completion state of query preparation.
    pub completion: Completion,
}

/// Edit preparation results; only a validated capability authorizes application.
#[derive(Debug)]
pub struct EditReport {
    /// Captured original snapshot associated with the prepared result.
    pub snapshot: ProjectSnapshot,

    /// Snapshots retained to resolve diagnostic and proposal locations.
    pub snapshots: Vec<ProjectSnapshot>,

    /// Linked editing locations when that query completes successfully.
    pub linked_ranges: Vec<instar_shared::Location>,

    /// Prepared proposal, which alone does not authorize application.
    pub proposal: Option<EditProposal>,

    /// Capability returned after required validation completes.
    pub validated: Option<ValidatedEdit>,

    /// Consulted input guards for freshness and application checks.
    pub inputs: Vec<InputVersion>,

    /// Explanations of prerequisites or guarantees that could not be established.
    pub unavailable_guarantees: Vec<String>,

    /// Diagnostics retained during preparation and validation.
    pub diagnostics: Vec<Diagnostic>,

    /// Completion state of edit preparation.
    pub completion: Completion,
}

/// Outcomes of explicitly authorized validated-edit application.
#[derive(Debug)]
pub struct ApplyReport {
    /// Identity of the proposal whose application was requested.
    pub identity: EditIdentity,

    /// Per-source write or nonapplication outcomes.
    pub writes: Vec<WriteOutcome>,

    /// Source revisions produced by applied changes.
    pub revisions: Vec<SourceVersion>,

    /// Snapshot captured for the application outcome.
    pub snapshot: ProjectSnapshot,

    /// Combined completion state of requested application.
    pub completion: Completion,
}

/// Captured effective settings and their schema identity.
#[derive(Debug)]
pub struct SettingsReport {
    /// Snapshot used to resolve the selected source settings.
    pub snapshot: ProjectSnapshot,

    /// Consulted input guards for freshness checks.
    pub inputs: Vec<InputVersion>,

    /// Operation whose effective settings were requested.
    pub operation: Operation,

    /// Effective settings and provenance for selected inputs.
    pub settings: Vec<EffectiveSettings>,

    /// Identifier of the effective-settings report schema.
    pub schema_identity: String,

    /// Diagnostics retained during settings capture.
    pub diagnostics: Vec<Diagnostic>,

    /// Completion state of settings capture.
    pub completion: Completion,
}

/// Public description of a catalogued lint rule.
#[derive(Clone, Debug, Serialize)]
pub struct RuleDescription {
    /// Stable catalog rule identity.
    pub identity: RuleIdentity,

    /// Short catalog description.
    pub description: String,

    /// Catalog-provided documentation.
    pub documentation: String,

    /// Whether the catalog provides fix support for this rule.
    pub fixes: bool,
}

/// A source location linked to an optional resolved destination.
#[derive(Clone, Debug, Serialize)]
pub struct ProjectLink {
    /// Exact source revision and byte range of the link.
    pub location: instar_shared::Location,

    /// Captured destination source identity, when available.
    pub target: Option<SourceIdentity>,

    /// Resolved destination address, when available.
    pub address: Option<String>,
}

/// One finding attributed to its analyzed module context.
#[derive(Clone, Debug, Serialize)]
pub struct ModuleDiagnostic {
    /// Module whose analysis produced the finding.
    pub module: ModuleIdentity,

    /// Finding with its exact source revision.
    pub diagnostic: Diagnostic,
}

/// Resolution evidence for one captured import site.
#[derive(Clone, Debug, Serialize)]
pub struct ImportExplanation {
    /// Module containing the import.
    pub importer: ModuleIdentity,

    /// Exact captured call, argument, and request.
    pub site: ImportSite,

    /// Resolved, unavailable, or nonconstant request.
    pub outcome: ImportOutcome,

    /// Actual resolution failure when resolution failed.
    pub failure: Option<ImportFailure>,

    /// Inputs consulted while resolving this site.
    pub inputs: Vec<InputVersion>,
}

/// The actual failed resolution operation.
#[derive(Clone, Debug)]
pub struct ImportFailure {
    /// Filesystem or resolution failure category.
    pub kind: io::ErrorKind,

    /// Failure explanation supplied by the resolver.
    pub message: String,
}

impl Serialize for ImportFailure {
    fn serialize<Encoder: Serializer>(
        &self,
        encoder: Encoder,
    ) -> Result<Encoder::Ok, Encoder::Error> {
        let mut failure = encoder.serialize_struct("ImportFailure", 2)?;
        failure.serialize_field("kind", &format!("{:?}", self.kind))?;
        failure.serialize_field("message", &self.message)?;

        failure.end()
    }
}
