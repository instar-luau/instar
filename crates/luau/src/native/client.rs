use std::{collections::BTreeMap, io, sync::Arc};

use instar_shared::{
    Availability, Completion, Location, Options, Reason, Span,
    process::{Outcome, Process},
};

use super::{
    analysis::{AnalysisPurpose, AnalysisRequest, AnalysisResult, ModuleAnalysis},
    completion::{CompletionContext, Completions},
    environment::EnvironmentMetadata,
    linting::{LintInput, LintResult, WarningAvailability},
    revision::AnalysisReference,
    semantics::{
        AnalysisIdentity, FactsInput, FactsResult, InspectionResult, PositionInput,
        SemanticAvailability, SemanticFacts,
    },
    source::ParsedMetadata,
};

use crate::{
    conversion::{self, Captured},
    flags::LanguageProfile,
    worker::protocol::{
        Operation, Request, Response, ResponseOutcome, WireAnalysisRequest, WireFactsInput,
        WireLintInput, WirePositionInput, WireReference,
    },
};

struct Established {
    captured: Arc<Captured>,
    revision: u64,
    generation: u64,
    modules: BTreeMap<AnalysisIdentity, SemanticAvailability>,
}

/// A lazy sequential client for captured native analysis and revision-guarded queries.
#[derive(Default)]
pub struct Native {
    process: Process<Request, Response>,
    profile: Option<LanguageProfile>,
    established: Option<Established>,
}

impl Native {
    /// Establishes the supplied closure without native filesystem discovery.
    ///
    /// # Errors
    /// Returns invalid captured input, profile, startup, native, framing or cleanup failures.
    pub fn analyze(
        &mut self,
        input: &AnalysisRequest,
        options: &Options,
    ) -> io::Result<AnalysisResult> {
        let captured = Arc::new(Captured::analysis(Arc::new(input.clone()))?);
        self.validate_captured(&captured)?;

        if self
            .profile
            .as_ref()
            .is_some_and(|profile| profile != &input.profile)
        {
            self.established = None;
            self.process.discard()?;
        }

        self.profile = Some(input.profile.clone());

        let request = WireAnalysisRequest::new(&captured)?;

        let output = match self.exchange(Operation::Analyze(request), options)? {
            Outcome::Response(response) => match response.outcome {
                ResponseOutcome::Analyzed(output) => output,

                _ => {
                    return self.reject(conversion::invalid(
                        "native worker returned a different operation result",
                    ));
                }
            },

            Outcome::Interrupted(completion) => return Ok(interrupted_analysis(input, completion)),
        };

        if let Err(error) = conversion::validate_analysis_result(&output, input, &captured) {
            return self.reject(error);
        }

        let interrupted = matches!(
            output.completion,
            Completion::Cancelled | Completion::TimedOut
        );

        let generation = self.process.generation();

        if !interrupted && generation.is_none() {
            return self.reject(conversion::invalid(
                "native analysis response has no live generation",
            ));
        }

        if let Some(previous) = &self.established
            && previous.generation == generation.unwrap_or(0)
            && (output.frontend_revision < previous.revision
                || output.frontend_revision == previous.revision
                    && previous.captured.analysis.as_deref() != Some(input))
        {
            return self.reject(conversion::invalid(
                "native frontend revision did not advance before changing captured inputs",
            ));
        }

        let modules = output
            .modules
            .into_iter()
            .map(|module| ModuleAnalysis {
                analysis: module.analysis,
                reference: generation
                    .filter(|_| !interrupted && module.parsed.completion == Completion::Complete)
                    .map(|generation| AnalysisReference {
                        analysis: module.analysis,
                        client: self.process.identity(),
                        generation,
                        frontend_revision: output.frontend_revision,
                        purpose: input.purpose,
                    }),
                parsed: module.parsed,
                environment: module.environment,
                diagnostics: module.diagnostics,
                availability: module.availability,
                completion: module.completion,
            })
            .collect::<Vec<_>>();

        if let Some(generation) = generation.filter(|_| !interrupted) {
            self.established = Some(Established {
                captured,
                revision: output.frontend_revision,
                generation,
                modules: modules
                    .iter()
                    .filter(|module| module.reference.is_some())
                    .map(|module| (module.analysis, module.availability.clone()))
                    .collect(),
            });
        }

        Ok(AnalysisResult {
            closure: output.closure,
            modules,
            completion: output.completion,
        })
    }

    /// Detaches requested semantic records from the exact referenced native state.
    ///
    /// # Errors
    /// Returns stale or foreign references, invalid requests, native, transport or cleanup failures.
    pub fn facts(&mut self, input: &FactsInput, options: &Options) -> io::Result<FactsResult> {
        let captured = Arc::clone(&self.validate(&input.analysis)?.captured);

        let request = WireFactsInput {
            analysis: wire_reference(&input.analysis),
            categories: input.categories.clone(),
        };

        conversion::facts_input(&request)?;

        let result = match self.exchange(Operation::Facts(request.clone()), options)? {
            Outcome::Response(response) => match response.outcome {
                ResponseOutcome::Facts(result) => result,

                _ => {
                    return self.reject(conversion::invalid(
                        "native worker returned a different operation result",
                    ));
                }
            },

            Outcome::Interrupted(completion) => {
                return Ok(FactsResult {
                    analysis: input.analysis.analysis,
                    facts: SemanticFacts::default(),
                    availability: SemanticAvailability {
                        categories: input
                            .categories
                            .iter()
                            .map(|category| {
                                (
                                    *category,
                                    Availability::Unavailable(Reason::FailedPrerequisite),
                                )
                            })
                            .collect(),
                    },
                    diagnostics: Vec::new(),
                    completion,
                });
            }
        };

        if result.analysis != input.analysis.analysis {
            return self.reject(conversion::invalid(
                "native facts identify different analysis",
            ));
        }

        if let Err(error) = conversion::validate_facts_result(&result, &request, &captured) {
            return self.reject(error);
        }

        Ok(result)
    }

    /// Runs selected upstream warnings without implicit inference.
    ///
    /// # Errors
    /// Returns stale references, invalid selections, native, transport or cleanup failures.
    pub fn lint(&mut self, input: &LintInput, options: &Options) -> io::Result<LintResult> {
        let captured = Arc::clone(&self.validate(&input.analysis)?.captured);

        let source = captured
            .module(input.analysis.analysis)?
            .source
            .source
            .version;

        let request = WireLintInput {
            analysis: wire_reference(&input.analysis),
            warnings: input.warnings.clone(),
        };

        conversion::lint_input(&request)?;

        let result = match self.exchange(Operation::Lint(request.clone()), options)? {
            Outcome::Response(response) => match response.outcome {
                ResponseOutcome::Linted(result) => result,

                _ => {
                    return self.reject(conversion::invalid(
                        "native worker returned a different operation result",
                    ));
                }
            },

            Outcome::Interrupted(completion) => {
                return Ok(LintResult {
                    source,
                    findings: Vec::new(),
                    suppression_findings: Vec::new(),
                    prerequisite_diagnostics: Vec::new(),
                    warnings: input
                        .warnings
                        .enabled
                        .iter()
                        .map(|warning| WarningAvailability {
                            warning: warning.clone(),
                            status: Availability::Unavailable(Reason::FailedPrerequisite),
                        })
                        .collect(),
                    completion,
                });
            }
        };

        if let Err(error) = conversion::validate_lint_result(&result, &request, &captured) {
            return self.reject(error);
        }

        Ok(result)
    }

    /// Inspects a UTF-8 byte cursor using established inference only.
    ///
    /// # Errors
    /// Returns stale references, invalid coordinates, unavailable inference or worker failures.
    pub fn inspect(
        &mut self,
        input: &PositionInput,
        options: &Options,
    ) -> io::Result<InspectionResult> {
        let established = self.validate(&input.analysis)?;
        let captured = Arc::clone(&established.captured);

        let availability = established
            .modules
            .get(&input.analysis.analysis)
            .cloned()
            .ok_or_else(|| conversion::invalid("native analysis availability is missing"))?;

        let request = WirePositionInput {
            analysis: wire_reference(&input.analysis),
            offset: input.offset,
        };

        conversion::position_input(&request, &captured)?;

        let result = match self.exchange(Operation::Inspect(request.clone()), options)? {
            Outcome::Response(response) => match response.outcome {
                ResponseOutcome::Inspected(result) => result,

                _ => {
                    return self.reject(conversion::invalid(
                        "native worker returned a different operation result",
                    ));
                }
            },

            Outcome::Interrupted(completion) => {
                return Ok(InspectionResult {
                    analysis: input.analysis.analysis,
                    occurrence: None,
                    availability: conversion::result_availability(&availability, &completion),
                    diagnostics: Vec::new(),
                    completion,
                });
            }
        };

        if result.analysis != input.analysis.analysis {
            return self.reject(conversion::invalid(
                "native inspection identifies different analysis",
            ));
        }

        if let Err(error) = conversion::validate_inspection_result(&result, &request, &captured) {
            return self.reject(error);
        }

        Ok(result)
    }

    /// Completes a UTF-8 byte cursor in explicitly established editor state.
    ///
    /// # Errors
    /// Returns stale references, invalid coordinates, missing editor caches or worker failures.
    pub fn complete(
        &mut self,
        input: &PositionInput,
        options: &Options,
    ) -> io::Result<Completions> {
        let captured = Arc::clone(&self.validate(&input.analysis)?.captured);

        if input.analysis.purpose != AnalysisPurpose::Editor {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "completion requires established editor analysis",
            ));
        }

        let request = WirePositionInput {
            analysis: wire_reference(&input.analysis),
            offset: input.offset,
        };

        conversion::position_input(&request, &captured)?;

        let result = match self.exchange(Operation::Complete(request.clone()), options)? {
            Outcome::Response(response) => match response.outcome {
                ResponseOutcome::Completed(result) => result,

                _ => {
                    return self.reject(conversion::invalid(
                        "native worker returned a different operation result",
                    ));
                }
            },

            Outcome::Interrupted(completion) => {
                return Ok(Completions {
                    analysis: input.analysis.analysis,
                    replacement: Location::new(
                        &captured.module(input.analysis.analysis)?.source.source,
                        Span {
                            start: input.offset,
                            end: input.offset,
                        },
                    )?,
                    context: CompletionContext::Unknown,
                    candidates: Vec::new(),
                    operator: None,
                    require: false,
                    loader: false,
                    prefix: None,
                    diagnostics: Vec::new(),
                    completion,
                });
            }
        };

        if let Err(error) = conversion::validate_completions_result(&result, &request, &captured) {
            return self.reject(error);
        }

        Ok(result)
    }

    /// Stops acceptance, expires references, and terminates and reaps the worker.
    ///
    /// # Errors
    /// Returns worker termination, reaping or transport cleanup failures.
    pub fn finish(&mut self) -> io::Result<()> {
        self.established = None;

        self.process.finish()
    }

    fn validate_captured(&self, captured: &Captured) -> io::Result<()> {
        if let Some(previous) = &self.established {
            for source in captured.documents.values() {
                if previous
                    .captured
                    .documents
                    .values()
                    .any(|old| old.version == source.version && old != source)
                {
                    return Err(conversion::invalid(
                        "captured revision changed its exact snapshot",
                    ));
                }
            }

            for module in captured.modules.values() {
                if previous
                    .captured
                    .modules
                    .values()
                    .any(|old| old.analysis == module.analysis && old != module)
                {
                    return Err(conversion::invalid(
                        "logical analysis identity changed its captured inputs",
                    ));
                }
            }
        }

        Ok(())
    }

    fn validate(&self, reference: &AnalysisReference) -> io::Result<&Established> {
        let established = self
            .established
            .as_ref()
            .ok_or_else(|| Reason::StaleQuery.error())?;

        if reference.client != self.process.identity()
            || Some(reference.generation) != self.process.generation()
            || reference.generation != established.generation
            || reference.frontend_revision != established.revision
        {
            return Err(Reason::StaleQuery.error());
        }

        conversion::validate_reference(
            &wire_reference(reference),
            &established.captured,
            established.revision,
        )?;

        if !established.modules.contains_key(&reference.analysis) {
            return Err(Reason::StaleQuery.error());
        }

        Ok(established)
    }

    fn exchange(
        &mut self,
        operation: Operation,
        options: &Options,
    ) -> io::Result<Outcome<Response>> {
        let mut identities = None;

        let result = self.process.request_with(
            "analysis",
            options,
            |request, generation, remaining_budget| {
                identities = Some((request, generation));

                Request {
                    identity: request,
                    generation,
                    remaining_budget,
                    operation,
                }
            },
        );

        let outcome = match result {
            Ok(outcome) => outcome,

            Err(error) => {
                if self.process.generation().is_none() {
                    self.established = None;
                }

                return Err(error);
            }
        };

        if let Outcome::Response(response) = &outcome {
            if identities != Some((response.identity, response.generation)) {
                return self.reject(conversion::invalid(
                    "native terminal response identities disagree",
                ));
            }

            let completion = match &response.outcome {
                ResponseOutcome::Analyzed(output) => &output.completion,
                ResponseOutcome::Facts(output) => &output.completion,
                ResponseOutcome::Linted(output) => &output.completion,
                ResponseOutcome::Inspected(output) => &output.completion,
                ResponseOutcome::Completed(output) => &output.completion,
            };

            if matches!(completion, Completion::Cancelled | Completion::TimedOut) {
                self.established = None;
                self.process.discard()?;
            }
        } else {
            self.established = None;
        }

        Ok(outcome)
    }

    fn reject<Output>(&mut self, error: io::Error) -> io::Result<Output> {
        self.established = None;

        match self.process.discard() {
            Ok(()) => Err(error),

            Err(cleanup) => Err(io::Error::new(
                error.kind(),
                format!("{error}; worker cleanup failed: {cleanup}"),
            )),
        }
    }
}

fn wire_reference(reference: &AnalysisReference) -> WireReference {
    WireReference {
        analysis: reference.analysis,
        frontend_revision: reference.frontend_revision,
        purpose: reference.purpose,
    }
}

fn interrupted_analysis(input: &AnalysisRequest, completion: Completion) -> AnalysisResult {
    AnalysisResult {
        closure: input.closure,
        modules: input
            .modules
            .iter()
            .map(|module| ModuleAnalysis {
                analysis: module.analysis,
                reference: None,
                parsed: ParsedMetadata {
                    input: module.source.clone(),
                    diagnostics: Vec::new(),
                    directives: Vec::new(),
                    imports: module
                        .imports
                        .iter()
                        .map(|import| import.site.clone())
                        .collect(),
                    recovered_syntax: false,
                    completion: completion.clone(),
                },
                environment: module
                    .environment
                    .as_ref()
                    .map(|environment| EnvironmentMetadata {
                        input: environment.clone(),
                        diagnostics: Vec::new(),
                        availability: Availability::Unavailable(Reason::FailedPrerequisite),
                        completion: completion.clone(),
                    }),
                diagnostics: Vec::new(),
                availability: SemanticAvailability {
                    categories: module
                        .inference
                        .retained
                        .iter()
                        .map(|category| {
                            (
                                *category,
                                Availability::Unavailable(Reason::FailedPrerequisite),
                            )
                        })
                        .collect(),
                },
                completion: completion.clone(),
            })
            .collect(),
        completion,
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, io, sync::Arc, time::Duration};

    use instar_shared::{
        Availability, Completion, ContextIdentity, Diagnostic, DiagnosticCategory, DiagnosticTag,
        FindingIdentity, Location, Reason, RelatedLocation, Severity, SourceEncoding,
        SourceIdentity, SourceSnapshot, SourceVersion, Span,
        process::{Message, RequestFrame},
    };

    use crate::{
        conversion::{self, Captured},
        flags::LanguageProfile,
        native::{
            AnalysisIdentity, AnalysisPurpose, AnalysisRequest, Capabilities, ClassCapability,
            ClosureIdentity, DeclarationGroup, Definition, EnvironmentIdentity, EnvironmentInput,
            EnvironmentMetadata, InferenceMode, InferenceSettings, InstanceEnvironment,
            InstanceRecord, LanguageFeatures, ModuleIdentity, ModuleInput, ParsedMetadata,
            PropertyCapability, ReportingSettings, SemanticAvailability, SemanticCategory,
            SourceDirective, SourceInput,
        },
        worker::protocol::{
            Operation, Request, Response, ResponseOutcome, WireAnalysisRequest, WireAnalysisResult,
            WireModuleAnalysis,
        },
    };

    fn repeated_request(text: &str) -> io::Result<AnalysisRequest> {
        let source = SourceSnapshot::new(
            SourceVersion {
                identity: SourceIdentity::new(1),
                revision: 7,
            },
            "\u{feff}return Shared",
        )?;

        let declaration = SourceSnapshot::new(
            SourceVersion {
                identity: SourceIdentity::new(2),
                revision: 9,
            },
            text,
        )?;

        let modules = (1..=40)
            .map(|identity| {
                let module = ModuleIdentity {
                    context: ContextIdentity::new(identity),
                    source: source.version.identity,
                };

                ModuleInput {
                    analysis: AnalysisIdentity {
                        module,
                        revision: 11,
                    },
                    source: SourceInput {
                        module,
                        source: source.clone(),
                        language: LanguageFeatures::default(),
                    },
                    environment: Some(repeated_environment(identity, &declaration)),
                    inference: InferenceSettings {
                        required: true,
                        mode: InferenceMode::Strict,
                        retained: vec![SemanticCategory::Types],
                    },
                    reporting: ReportingSettings {
                        ordinary_type_diagnostics: true,
                    },
                    imports: Vec::new(),
                }
            })
            .collect::<Vec<_>>();

        Ok(AnalysisRequest {
            closure: ClosureIdentity::new(13),
            profile: LanguageProfile::new(&BTreeMap::new(), 64 * 1024 * 1024)?,
            purpose: AnalysisPurpose::Editor,
            roots: modules
                .iter()
                .map(|module| module.analysis.module)
                .collect(),
            modules,
            cycles: Vec::new(),
        })
    }

    fn repeated_environment(identity: u64, declaration: &SourceSnapshot) -> EnvironmentInput {
        let script = format!("script{identity}");

        EnvironmentInput {
            identity: EnvironmentIdentity::new(identity),
            language: LanguageFeatures {
                declaration_syntax: true,
                ..LanguageFeatures::default()
            },
            globals: vec!["Shared".to_owned()],
            declarations: vec![
                DeclarationGroup::default(),
                DeclarationGroup {
                    definitions: vec![Definition {
                        source: declaration.clone(),
                        namespace: format!("@first{identity}"),
                    }],
                },
                DeclarationGroup {
                    definitions: vec![Definition {
                        source: declaration.clone(),
                        namespace: format!("@second{identity}"),
                    }],
                },
            ],
            capabilities: Capabilities {
                classes: vec![ClassCapability {
                    name: "Part".to_owned(),
                    service: false,
                    creatable: true,
                    properties: vec![PropertyCapability {
                        name: "Name".to_owned(),
                        read: true,
                        write: identity % 2 == 0,
                    }],
                }],
            },
            instances: Some(InstanceEnvironment {
                root: "root".to_owned(),
                script: script.clone(),
                instances: vec![
                    InstanceRecord {
                        identity: "root".to_owned(),
                        name: "game".to_owned(),
                        class: "DataModel".to_owned(),
                        parent: None,
                        children: vec![script.clone()],
                    },
                    InstanceRecord {
                        identity: script,
                        name: "Script".to_owned(),
                        class: "ModuleScript".to_owned(),
                        parent: Some("root".to_owned()),
                        children: Vec::new(),
                    },
                ],
            }),
        }
    }

    fn repeated_response(captured: &Captured) -> io::Result<WireAnalysisResult> {
        let input = captured.analysis.as_ref().expect("captured analysis");

        let modules = input
            .modules
            .iter()
            .map(|module| {
                let primary = Location::new(
                    &module.source.source,
                    Span {
                        start: 0,
                        end: module.source.source.text.len(),
                    },
                )?;

                let diagnostic = Diagnostic {
                    identity: FindingIdentity::new(
                        "luau",
                        captured.inputs(),
                        "SyntaxError",
                        primary,
                    ),
                    code: "SyntaxError".to_owned(),
                    category: DiagnosticCategory::Syntax,
                    severity: Severity::Error,
                    message: format!("module {:?} recovered syntax", module.analysis.module),
                    primary,
                    related: vec![RelatedLocation {
                        location: primary,
                        message: "captured source".to_owned(),
                    }],
                    tags: vec![DiagnosticTag::Deprecated],
                };

                Ok(WireModuleAnalysis {
                    analysis: module.analysis,
                    parsed: ParsedMetadata {
                        input: module.source.clone(),
                        diagnostics: vec![diagnostic.clone()],
                        directives: vec![SourceDirective {
                            location: primary,
                            text: "--!strict".to_owned(),
                        }],
                        imports: module
                            .imports
                            .iter()
                            .map(|import| import.site.clone())
                            .collect(),
                        recovered_syntax: true,
                        completion: Completion::Complete,
                    },
                    environment: module.environment.as_ref().map(|environment| {
                        EnvironmentMetadata {
                            input: environment.clone(),
                            diagnostics: vec![diagnostic.clone()],
                            availability: Availability::Partial(Reason::UnavailableEnvironment),
                            completion: Completion::Incomplete(vec![
                                Reason::UnavailableEnvironment,
                            ]),
                        }
                    }),
                    diagnostics: vec![diagnostic],
                    availability: SemanticAvailability {
                        categories: [(
                            SemanticCategory::Types,
                            Availability::Partial(Reason::UnsupportedSyntax),
                        )]
                        .into_iter()
                        .collect(),
                    },
                    completion: Completion::Incomplete(vec![Reason::UnavailableSemantics]),
                })
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(WireAnalysisResult {
            closure: input.closure,
            frontend_revision: 17,
            modules,
            completion: Completion::Incomplete(vec![Reason::UnavailableSemantics]),
        })
    }

    #[test]
    fn repeated_declarations_fit_the_request_frame() -> io::Result<()> {
        let text = format!(
            "\u{feff}declare Shared: number\n--[[{}]]",
            "x".repeat(1024 * 1024)
        );

        let input = repeated_request(&text)?;
        let captured = Captured::analysis(Arc::new(input.clone()))?;
        let limit = 64 * 1024 * 1024;
        assert!(serde_json::to_vec(&input)?.len() > limit);

        let frame = RequestFrame {
            request: 1,
            generation: 1,
            invocation: 1,
            payload: Request {
                identity: 1,
                generation: 1,
                remaining_budget: Some(Duration::from_secs(5)),
                operation: Operation::Analyze(WireAnalysisRequest::new(&captured)?),
            },
        };

        let mut serialized = serde_json::to_vec(&frame)?;
        serialized.push(b'\n');

        assert!(
            serialized.len() <= limit,
            "request frame is {} bytes",
            serialized.len()
        );

        let decoded: RequestFrame<Request> = serde_json::from_slice(&serialized)?;
        assert_eq!(decoded.request, frame.request);
        assert_eq!(decoded.generation, frame.generation);
        assert_eq!(decoded.invocation, frame.invocation);
        assert_eq!(decoded.payload.identity, frame.payload.identity);
        assert_eq!(decoded.payload.generation, frame.payload.generation);

        assert_eq!(
            decoded.payload.remaining_budget,
            frame.payload.remaining_budget
        );

        let Operation::Analyze(wire) = decoded.payload.operation else {
            panic!("analysis request changed its operation");
        };

        assert_eq!(wire.snapshots.len(), 2);
        let reconstructed = wire.unpack()?;
        assert_eq!(reconstructed, input);
        Captured::analysis(Arc::new(reconstructed))?;

        Ok(())
    }

    #[test]
    fn repeated_declarations_fit_the_response_frame() -> io::Result<()> {
        let text = format!(
            "\u{feff}declare Shared: number\n--[[{}]]",
            "x".repeat(1024 * 1024)
        );

        let input = repeated_request(&text)?;
        let captured = Captured::analysis(Arc::new(input.clone()))?;
        let output = repeated_response(&captured)?;
        conversion::validate_analysis_result(&output, &input, &captured)?;

        let frame = Message::Result {
            request: 19,
            generation: 23,
            result: Ok::<_, serde_json::Value>(Response {
                identity: 19,
                generation: 23,
                outcome: ResponseOutcome::Analyzed(output),
            }),
        };

        let mut serialized = serde_json::to_vec(&frame)?;
        serialized.push(b'\n');
        let limit = 64 * 1024 * 1024;

        assert!(
            serialized.len() <= limit,
            "response frame is {} bytes",
            serialized.len()
        );

        let decoded: Message<Result<Response, serde_json::Value>> =
            serde_json::from_slice(&serialized)?;

        let Message::Result {
            request,
            generation,
            result,
        } = decoded
        else {
            panic!("analysis response changed its frame");
        };

        assert_eq!(request, 19);
        assert_eq!(generation, 23);
        let response = result.expect("successful analysis response");
        assert_eq!(response.identity, request);
        assert_eq!(response.generation, generation);

        let ResponseOutcome::Analyzed(reconstructed) = response.outcome else {
            panic!("analysis response changed its outcome");
        };

        conversion::validate_analysis_result(&reconstructed, &input, &captured)?;

        let Message::Result { result, .. } = frame else {
            panic!("analysis response frame");
        };

        let ResponseOutcome::Analyzed(output) = result.expect("analysis result").outcome else {
            panic!("analysis response outcome");
        };

        assert_eq!(reconstructed.closure, output.closure);
        assert_eq!(reconstructed.frontend_revision, output.frontend_revision);
        assert_eq!(reconstructed.completion, output.completion);
        assert_eq!(reconstructed.modules.len(), output.modules.len());

        for (reconstructed, output) in reconstructed.modules.iter().zip(&output.modules) {
            assert_eq!(reconstructed.analysis, output.analysis);
            assert_eq!(reconstructed.parsed, output.parsed);
            assert_eq!(reconstructed.environment, output.environment);
            assert_eq!(reconstructed.diagnostics, output.diagnostics);
            assert_eq!(reconstructed.availability, output.availability);
            assert_eq!(reconstructed.completion, output.completion);
        }

        Ok(())
    }

    #[test]
    fn snapshot_table_rejects_invalid_references_and_rows() -> io::Result<()> {
        let input = repeated_request("declare Shared: number")?;
        let captured = Captured::analysis(Arc::new(input))?;

        let mut wire = WireAnalysisRequest::new(&captured)?;
        wire.modules[0].source.source = wire.snapshots.len();
        assert!(wire.unpack().is_err());

        let mut wire = WireAnalysisRequest::new(&captured)?;

        wire.modules[0]
            .environment
            .as_mut()
            .expect("environment")
            .declarations[1]
            .definitions[0]
            .source = wire.snapshots.len();

        assert!(wire.unpack().is_err());

        let mut wire = WireAnalysisRequest::new(&captured)?;
        wire.snapshots.push(wire.snapshots[0].clone());
        assert!(wire.unpack().is_err());

        let mut wire = WireAnalysisRequest::new(&captured)?;
        let mut conflicting = wire.snapshots[0].clone();
        conflicting.version.revision += 1;
        wire.snapshots.push(conflicting);
        assert!(wire.unpack().is_err());

        let mut wire = WireAnalysisRequest::new(&captured)?;
        let mut unused = wire.snapshots[1].clone();
        unused.version.identity = SourceIdentity::new(3);
        wire.snapshots.push(unused);
        assert!(wire.unpack().is_err());

        let mut wire = WireAnalysisRequest::new(&captured)?;
        let mut malformed = wire.snapshots[1].clone();
        malformed.version.identity = SourceIdentity::new(3);
        malformed.encoding = SourceEncoding::Utf8Marker;
        wire.snapshots.push(malformed);
        assert!(wire.unpack().is_err());

        Ok(())
    }
}
