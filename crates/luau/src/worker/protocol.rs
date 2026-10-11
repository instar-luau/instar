use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Duration,
};

use instar_shared::{Availability, Completion, Diagnostic, SourceSnapshot, SourceVersion};
use serde::{Deserialize, Serialize, Serializer, ser::Error};

use crate::{
    conversion,
    flags::LanguageProfile,
    native::{
        AnalysisIdentity, AnalysisPurpose, AnalysisRequest, Capabilities, ClosureIdentity,
        Completions, CyclePath, DeclarationGroup, Definition, EnvironmentIdentity,
        EnvironmentInput, EnvironmentMetadata, FactsResult, ImportSite, InferenceSettings,
        InspectionResult, InstanceEnvironment, LanguageFeatures, LintResult, ModuleIdentity,
        ModuleInput, ParsedMetadata, ReportingSettings, ResolvedImport, SemanticAvailability,
        SemanticCategory, SourceDirective, SourceInput, UpstreamWarningSettings,
    },
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
    Analyze(WireAnalysisRequest),
    Facts(WireFactsInput),
    Lint(WireLintInput),
    Inspect(WirePositionInput),
    Complete(WirePositionInput),
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireAnalysisRequest {
    pub closure: ClosureIdentity,
    pub profile: LanguageProfile,
    pub purpose: AnalysisPurpose,
    pub roots: Vec<ModuleIdentity>,
    pub modules: Vec<WireModuleInput>,
    pub cycles: Vec<CyclePath>,
    pub snapshots: Vec<SourceSnapshot>,
}

impl WireAnalysisRequest {
    pub(crate) fn new(captured: &conversion::Captured) -> io::Result<Self> {
        let input = captured
            .analysis
            .as_ref()
            .ok_or_else(|| conversion::invalid("native captured analysis input is missing"))?;

        let mut snapshots = Vec::new();
        let mut indices = BTreeMap::new();

        let modules = input
            .modules
            .iter()
            .map(|module| {
                let source = WireSourceInput::new(&module.source, &mut snapshots, &mut indices)?;

                let environment = module
                    .environment
                    .as_ref()
                    .map(|environment| {
                        WireEnvironmentInput::new(environment, &mut snapshots, &mut indices)
                    })
                    .transpose()?;

                Ok(WireModuleInput {
                    analysis: module.analysis,
                    source,
                    environment,
                    inference: module.inference.clone(),
                    reporting: module.reporting.clone(),
                    imports: module.imports.clone(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(Self {
            closure: input.closure,
            profile: input.profile.clone(),
            purpose: input.purpose,
            roots: input.roots.clone(),
            modules,
            cycles: input.cycles.clone(),
            snapshots,
        })
    }

    pub(crate) fn unpack(&self) -> io::Result<AnalysisRequest> {
        validate_snapshots(&self.snapshots)?;
        let mut used = BTreeSet::new();

        let modules = self
            .modules
            .iter()
            .map(|module| {
                let source = module.source.unpack(&self.snapshots, &mut used)?;

                let environment = module
                    .environment
                    .as_ref()
                    .map(|environment| environment.unpack(&self.snapshots, &mut used))
                    .transpose()?;

                Ok(ModuleInput {
                    analysis: module.analysis,
                    source,
                    environment,
                    inference: module.inference.clone(),
                    reporting: module.reporting.clone(),
                    imports: module.imports.clone(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        validate_snapshot_usage(&self.snapshots, &used)?;

        Ok(AnalysisRequest {
            closure: self.closure,
            profile: self.profile.clone(),
            purpose: self.purpose,
            roots: self.roots.clone(),
            modules,
            cycles: self.cycles.clone(),
        })
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireModuleInput {
    pub analysis: AnalysisIdentity,
    pub source: WireSourceInput,
    pub environment: Option<WireEnvironmentInput>,
    pub inference: InferenceSettings,
    pub reporting: ReportingSettings,
    pub imports: Vec<ResolvedImport>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireSourceInput {
    pub module: ModuleIdentity,
    pub source: usize,
    pub language: LanguageFeatures,
}

impl WireSourceInput {
    fn new(
        input: &SourceInput,
        snapshots: &mut Vec<SourceSnapshot>,
        indices: &mut BTreeMap<SourceVersion, usize>,
    ) -> io::Result<Self> {
        Ok(Self {
            module: input.module,
            source: snapshot_index(&input.source, snapshots, indices)?,
            language: input.language.clone(),
        })
    }

    fn unpack(
        &self,
        snapshots: &[SourceSnapshot],
        used: &mut BTreeSet<usize>,
    ) -> io::Result<SourceInput> {
        Ok(SourceInput {
            module: self.module,
            source: snapshot(snapshots, self.source, used)?,
            language: self.language.clone(),
        })
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireEnvironmentInput {
    pub identity: EnvironmentIdentity,
    pub language: LanguageFeatures,
    pub globals: Vec<String>,
    pub declarations: Vec<WireDeclarationGroup>,
    pub capabilities: Capabilities,
    pub instances: Option<InstanceEnvironment>,
}

impl WireEnvironmentInput {
    fn new(
        input: &EnvironmentInput,
        snapshots: &mut Vec<SourceSnapshot>,
        indices: &mut BTreeMap<SourceVersion, usize>,
    ) -> io::Result<Self> {
        let declarations = input
            .declarations
            .iter()
            .map(|group| {
                let definitions = group
                    .definitions
                    .iter()
                    .map(|definition| {
                        Ok(WireDefinition {
                            source: snapshot_index(&definition.source, snapshots, indices)?,
                            namespace: definition.namespace.clone(),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;

                Ok(WireDeclarationGroup { definitions })
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(Self {
            identity: input.identity,
            language: input.language.clone(),
            globals: input.globals.clone(),
            declarations,
            capabilities: input.capabilities.clone(),
            instances: input.instances.clone(),
        })
    }

    fn unpack(
        &self,
        snapshots: &[SourceSnapshot],
        used: &mut BTreeSet<usize>,
    ) -> io::Result<EnvironmentInput> {
        let declarations = self
            .declarations
            .iter()
            .map(|group| {
                let definitions = group
                    .definitions
                    .iter()
                    .map(|definition| {
                        Ok(Definition {
                            source: snapshot(snapshots, definition.source, used)?,
                            namespace: definition.namespace.clone(),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;

                Ok(DeclarationGroup { definitions })
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(EnvironmentInput {
            identity: self.identity,
            language: self.language.clone(),
            globals: self.globals.clone(),
            declarations,
            capabilities: self.capabilities.clone(),
            instances: self.instances.clone(),
        })
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireDeclarationGroup {
    pub definitions: Vec<WireDefinition>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireDefinition {
    pub source: usize,
    pub namespace: String,
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

pub(crate) struct WireModuleAnalysis {
    pub analysis: AnalysisIdentity,
    pub parsed: ParsedMetadata,
    pub environment: Option<EnvironmentMetadata>,
    pub diagnostics: Vec<Diagnostic>,
    pub availability: SemanticAvailability,
    pub completion: Completion,
}

#[derive(Deserialize)]
#[serde(try_from = "PackedAnalysisResult")]
pub(crate) struct WireAnalysisResult {
    pub closure: ClosureIdentity,
    pub frontend_revision: u64,
    pub modules: Vec<WireModuleAnalysis>,
    pub completion: Completion,
}

impl Serialize for WireAnalysisResult {
    fn serialize<Output>(&self, serializer: Output) -> Result<Output::Ok, Output::Error>
    where
        Output: Serializer,
    {
        PackedAnalysisResult::new(self)
            .map_err(Output::Error::custom)?
            .serialize(serializer)
    }
}

impl TryFrom<PackedAnalysisResult> for WireAnalysisResult {
    type Error = io::Error;

    fn try_from(input: PackedAnalysisResult) -> io::Result<Self> {
        validate_snapshots(&input.snapshots)?;
        let mut used = BTreeSet::new();

        let modules = input
            .modules
            .into_iter()
            .map(|module| {
                let parsed = ParsedMetadata {
                    input: module.parsed.input.unpack(&input.snapshots, &mut used)?,
                    diagnostics: module.parsed.diagnostics,
                    directives: module.parsed.directives,
                    imports: module.parsed.imports,
                    recovered_syntax: module.parsed.recovered_syntax,
                    completion: module.parsed.completion,
                };

                let environment = module
                    .environment
                    .map(|environment| -> io::Result<_> {
                        Ok(EnvironmentMetadata {
                            input: environment.input.unpack(&input.snapshots, &mut used)?,
                            diagnostics: environment.diagnostics,
                            availability: environment.availability,
                            completion: environment.completion,
                        })
                    })
                    .transpose()?;

                Ok(WireModuleAnalysis {
                    analysis: module.analysis,
                    parsed,
                    environment,
                    diagnostics: module.diagnostics,
                    availability: module.availability,
                    completion: module.completion,
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        validate_snapshot_usage(&input.snapshots, &used)?;

        Ok(Self {
            closure: input.closure,
            frontend_revision: input.frontend_revision,
            modules,
            completion: input.completion,
        })
    }
}

#[derive(Deserialize, Serialize)]
struct PackedAnalysisResult {
    closure: ClosureIdentity,
    frontend_revision: u64,
    modules: Vec<PackedModuleAnalysis>,
    completion: Completion,
    snapshots: Vec<SourceSnapshot>,
}

impl PackedAnalysisResult {
    fn new(input: &WireAnalysisResult) -> io::Result<Self> {
        let mut snapshots = Vec::new();
        let mut indices = BTreeMap::new();

        let modules = input
            .modules
            .iter()
            .map(|module| {
                let parsed = PackedParsedMetadata {
                    input: WireSourceInput::new(
                        &module.parsed.input,
                        &mut snapshots,
                        &mut indices,
                    )?,
                    diagnostics: module.parsed.diagnostics.clone(),
                    directives: module.parsed.directives.clone(),
                    imports: module.parsed.imports.clone(),
                    recovered_syntax: module.parsed.recovered_syntax,
                    completion: module.parsed.completion.clone(),
                };

                let environment = module
                    .environment
                    .as_ref()
                    .map(|environment| -> io::Result<_> {
                        Ok(PackedEnvironmentMetadata {
                            input: WireEnvironmentInput::new(
                                &environment.input,
                                &mut snapshots,
                                &mut indices,
                            )?,
                            diagnostics: environment.diagnostics.clone(),
                            availability: environment.availability,
                            completion: environment.completion.clone(),
                        })
                    })
                    .transpose()?;

                Ok(PackedModuleAnalysis {
                    analysis: module.analysis,
                    parsed,
                    environment,
                    diagnostics: module.diagnostics.clone(),
                    availability: module.availability.clone(),
                    completion: module.completion.clone(),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        validate_snapshots(&snapshots)?;

        Ok(Self {
            closure: input.closure,
            frontend_revision: input.frontend_revision,
            modules,
            completion: input.completion.clone(),
            snapshots,
        })
    }
}

#[derive(Deserialize, Serialize)]
struct PackedModuleAnalysis {
    analysis: AnalysisIdentity,
    parsed: PackedParsedMetadata,
    environment: Option<PackedEnvironmentMetadata>,
    diagnostics: Vec<Diagnostic>,
    availability: SemanticAvailability,
    completion: Completion,
}

#[derive(Deserialize, Serialize)]
struct PackedParsedMetadata {
    input: WireSourceInput,
    diagnostics: Vec<Diagnostic>,
    directives: Vec<SourceDirective>,
    imports: Vec<ImportSite>,
    recovered_syntax: bool,
    completion: Completion,
}

#[derive(Deserialize, Serialize)]
struct PackedEnvironmentMetadata {
    input: WireEnvironmentInput,
    diagnostics: Vec<Diagnostic>,
    availability: Availability,
    completion: Completion,
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

fn validate_snapshots(snapshots: &[SourceSnapshot]) -> io::Result<()> {
    let mut identities = BTreeSet::new();

    for snapshot in snapshots {
        snapshot.validate()?;

        if !identities.insert(snapshot.version.identity) {
            return Err(conversion::invalid(
                "native snapshot table has duplicate or conflicting source identities",
            ));
        }
    }

    Ok(())
}

fn validate_snapshot_usage(snapshots: &[SourceSnapshot], used: &BTreeSet<usize>) -> io::Result<()> {
    if used.len() != snapshots.len() {
        return Err(conversion::invalid("native snapshot table has unused rows"));
    }

    Ok(())
}

fn snapshot(
    snapshots: &[SourceSnapshot],
    index: usize,
    used: &mut BTreeSet<usize>,
) -> io::Result<SourceSnapshot> {
    let snapshot = snapshots
        .get(index)
        .ok_or_else(|| conversion::invalid("native snapshot reference is out of range"))?;

    used.insert(index);

    Ok(snapshot.clone())
}

fn snapshot_index(
    snapshot: &SourceSnapshot,
    snapshots: &mut Vec<SourceSnapshot>,
    indices: &mut BTreeMap<SourceVersion, usize>,
) -> io::Result<usize> {
    if let Some(&index) = indices.get(&snapshot.version) {
        if snapshots[index] != *snapshot {
            return Err(conversion::invalid(
                "native source revision has conflicting snapshots",
            ));
        }

        return Ok(index);
    }

    let index = snapshots.len();
    snapshots.push(snapshot.clone());
    indices.insert(snapshot.version, index);

    Ok(index)
}
