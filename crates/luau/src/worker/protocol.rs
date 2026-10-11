use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Duration,
};

use instar_shared::{Availability, Completion, Diagnostic, SourceSnapshot, SourceVersion};
use serde::{Deserialize, Serialize};

use crate::{
    conversion::{self, Captured},
    flags::LanguageProfile,
    native::{
        AnalysisIdentity, AnalysisPurpose, AnalysisRequest, Capabilities, ClosureIdentity,
        Completions, CyclePath, DeclarationGroup, Definition, EnvironmentIdentity,
        EnvironmentInput, EnvironmentMetadata, FactsResult, ImportSite, InferenceSettings,
        InspectionResult, InstanceEnvironment, InstanceRecord, LanguageFeatures, LintResult,
        ModuleIdentity, ModuleInput, ParsedMetadata, ReportingSettings, ResolvedImport,
        SemanticAvailability, SemanticCategory, SourceDirective, SourceInput,
        UpstreamWarningSettings,
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
    Capture {
        index: usize,
        snapshot: SourceSnapshot,
    },
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
    pub snapshots: Vec<SourceVersion>,
    pub capabilities: Vec<Capabilities>,
    pub instance_trees: Vec<Vec<InstanceRecord>>,
}

impl WireAnalysisRequest {
    pub(crate) fn new(captured: &Captured) -> io::Result<(Self, Vec<SourceSnapshot>)> {
        let input = captured
            .analysis
            .as_ref()
            .ok_or_else(|| conversion::invalid("native captured analysis input is missing"))?;

        let mut snapshots = Vec::new();
        let mut indices = BTreeMap::new();
        let mut capabilities = Vec::new();
        let mut instance_trees = Vec::new();

        let modules = input
            .modules
            .iter()
            .map(|module| {
                let source = WireSourceInput::new(&module.source, &mut snapshots, &mut indices)?;
                let environment = module
                    .environment
                    .as_ref()
                    .map(|environment| {
                        WireEnvironmentInput::new(
                            environment,
                            &mut snapshots,
                            &mut indices,
                            &mut capabilities,
                            &mut instance_trees,
                        )
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

        let request = Self {
            closure: input.closure,
            profile: input.profile.clone(),
            purpose: input.purpose,
            roots: input.roots.clone(),
            modules,
            cycles: input.cycles.clone(),
            snapshots: snapshots.iter().map(|snapshot| snapshot.version).collect(),
            capabilities,
            instance_trees,
        };

        Ok((request, snapshots))
    }

    pub(crate) fn unpack(&self, snapshots: &[SourceSnapshot]) -> io::Result<AnalysisRequest> {
        validate_snapshots(snapshots)?;

        if self.snapshots
            != snapshots
                .iter()
                .map(|snapshot| snapshot.version)
                .collect::<Vec<_>>()
        {
            return Err(conversion::invalid(
                "native capture does not match its snapshot table",
            ));
        }

        let mut used = BTreeSet::new();
        let mut used_capabilities = BTreeSet::new();
        let mut used_instance_trees = BTreeSet::new();

        let modules = self
            .modules
            .iter()
            .map(|module| {
                let source = module.source.unpack(snapshots, &mut used)?;
                let environment = module
                    .environment
                    .as_ref()
                    .map(|environment| {
                        environment.unpack(
                            snapshots,
                            &mut used,
                            &self.capabilities,
                            &mut used_capabilities,
                            &self.instance_trees,
                            &mut used_instance_trees,
                        )
                    })
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

        validate_usage(snapshots.len(), &used)?;
        validate_usage(self.capabilities.len(), &used_capabilities)?;
        validate_usage(self.instance_trees.len(), &used_instance_trees)?;

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
            source: row(snapshots, self.source, used)?.clone(),
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
    pub capabilities: usize,
    pub instances: Option<WireInstanceEnvironment>,
}

impl WireEnvironmentInput {
    fn new(
        input: &EnvironmentInput,
        snapshots: &mut Vec<SourceSnapshot>,
        indices: &mut BTreeMap<SourceVersion, usize>,
        capabilities: &mut Vec<Capabilities>,
        instance_trees: &mut Vec<Vec<InstanceRecord>>,
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

        let instances = input
            .instances
            .as_ref()
            .map(|instances| WireInstanceEnvironment {
                root: instances.root.clone(),
                script: instances.script.clone(),
                instances: table_index(&instances.instances, instance_trees),
            });

        Ok(Self {
            identity: input.identity,
            language: input.language.clone(),
            globals: input.globals.clone(),
            declarations,
            capabilities: table_index(&input.capabilities, capabilities),
            instances,
        })
    }

    fn unpack(
        &self,
        snapshots: &[SourceSnapshot],
        used: &mut BTreeSet<usize>,
        capabilities: &[Capabilities],
        used_capabilities: &mut BTreeSet<usize>,
        instance_trees: &[Vec<InstanceRecord>],
        used_instance_trees: &mut BTreeSet<usize>,
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
                            source: row(snapshots, definition.source, used)?.clone(),
                            namespace: definition.namespace.clone(),
                        })
                    })
                    .collect::<io::Result<Vec<_>>>()?;

                Ok(DeclarationGroup { definitions })
            })
            .collect::<io::Result<Vec<_>>>()?;

        let instances = self
            .instances
            .as_ref()
            .map(|instances| -> io::Result<_> {
                Ok(InstanceEnvironment {
                    root: instances.root.clone(),
                    script: instances.script.clone(),
                    instances: row(instance_trees, instances.instances, used_instance_trees)?
                        .clone(),
                })
            })
            .transpose()?;

        Ok(EnvironmentInput {
            identity: self.identity,
            language: self.language.clone(),
            globals: self.globals.clone(),
            declarations,
            capabilities: row(capabilities, self.capabilities, used_capabilities)?.clone(),
            instances,
        })
    }
}

#[derive(Deserialize, Serialize)]
pub(crate) struct WireInstanceEnvironment {
    pub root: String,
    pub script: String,
    pub instances: usize,
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

pub(crate) struct WireAnalysisResult {
    pub closure: ClosureIdentity,
    pub frontend_revision: u64,
    pub modules: Vec<WireModuleAnalysis>,
    pub completion: Completion,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct PackedAnalysisResult {
    pub closure: ClosureIdentity,
    pub frontend_revision: u64,
    modules: Vec<PackedModuleAnalysis>,
    pub completion: Completion,
}

impl PackedAnalysisResult {
    pub(crate) fn new(input: &WireAnalysisResult, captured: &Captured) -> io::Result<Self> {
        let request = captured
            .analysis
            .as_ref()
            .ok_or_else(|| conversion::invalid("native captured analysis input is missing"))?;
        conversion::validate_analysis_result(input, request, captured)?;

        let modules = input
            .modules
            .iter()
            .map(|module| PackedModuleAnalysis {
                analysis: module.analysis,
                parsed: PackedParsedMetadata {
                    source: module.parsed.input.source.version,
                    language: module.parsed.input.language.clone(),
                    diagnostics: module.parsed.diagnostics.clone(),
                    directives: module.parsed.directives.clone(),
                    imports: module.parsed.imports.clone(),
                    recovered_syntax: module.parsed.recovered_syntax,
                    completion: module.parsed.completion.clone(),
                },
                environment: module.environment.as_ref().map(|environment| {
                    PackedEnvironmentMetadata {
                        identity: environment.input.identity,
                        diagnostics: environment.diagnostics.clone(),
                        availability: environment.availability,
                        completion: environment.completion.clone(),
                    }
                }),
                diagnostics: module.diagnostics.clone(),
                availability: module.availability.clone(),
                completion: module.completion.clone(),
            })
            .collect();

        Ok(Self {
            closure: input.closure,
            frontend_revision: input.frontend_revision,
            modules,
            completion: input.completion.clone(),
        })
    }

    pub(crate) fn unpack(self, captured: &Captured) -> io::Result<WireAnalysisResult> {
        let modules = self.modules.into_iter().map(|module| {
            let input = captured.module(module.analysis)?;

            if module.parsed.source != input.source.source.version {
                return Err(conversion::invalid("native parsed source reference disagrees with its captured input"));
            }

            let environment = module.environment.map(|environment| -> io::Result<_> {
                let input = input.environment.as_ref()
                    .filter(|input| input.identity == environment.identity)
                    .ok_or_else(|| conversion::invalid("native environment reference disagrees with its captured input"))?;

                Ok(EnvironmentMetadata {
                    input: input.clone(),
                    diagnostics: environment.diagnostics,
                    availability: environment.availability,
                    completion: environment.completion,
                })
            }).transpose()?;

            let mut source = input.source.clone();
            source.language = module.parsed.language;

            Ok(WireModuleAnalysis {
                analysis: module.analysis,
                parsed: ParsedMetadata {
                    input: source,
                    diagnostics: module.parsed.diagnostics,
                    directives: module.parsed.directives,
                    imports: module.parsed.imports,
                    recovered_syntax: module.parsed.recovered_syntax,
                    completion: module.parsed.completion,
                },
                environment,
                diagnostics: module.diagnostics,
                availability: module.availability,
                completion: module.completion,
            })
        }).collect::<io::Result<Vec<_>>>()?;

        let output = WireAnalysisResult {
            closure: self.closure,
            frontend_revision: self.frontend_revision,
            modules,
            completion: self.completion,
        };

        let input = captured
            .analysis
            .as_ref()
            .ok_or_else(|| conversion::invalid("native captured analysis input is missing"))?;
        conversion::validate_analysis_result(&output, input, captured)?;

        Ok(output)
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
    source: SourceVersion,
    language: LanguageFeatures,
    diagnostics: Vec<Diagnostic>,
    directives: Vec<SourceDirective>,
    imports: Vec<ImportSite>,
    recovered_syntax: bool,
    completion: Completion,
}

#[derive(Deserialize, Serialize)]
struct PackedEnvironmentMetadata {
    identity: EnvironmentIdentity,
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
    Captured { index: usize, source: SourceVersion },
    Analyzed(PackedAnalysisResult),
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

fn validate_usage(length: usize, used: &BTreeSet<usize>) -> io::Result<()> {
    if used.len() != length {
        return Err(conversion::invalid("native input table has unused rows"));
    }

    Ok(())
}

fn row<'table, Value>(
    table: &'table [Value],
    index: usize,
    used: &mut BTreeSet<usize>,
) -> io::Result<&'table Value> {
    let value = table
        .get(index)
        .ok_or_else(|| conversion::invalid("native input reference is out of range"))?;
    used.insert(index);
    Ok(value)
}

fn table_index<Value: Clone + PartialEq>(value: &Value, table: &mut Vec<Value>) -> usize {
    if let Some(index) = table.iter().position(|entry| entry == value) {
        return index;
    }

    let index = table.len();
    table.push(value.clone());
    index
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
