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
        Operation, Request, Response, ResponseOutcome, WireFactsInput, WireLintInput,
        WirePositionInput, WireReference,
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

        let output = match self.exchange(Operation::Analyze(input.clone()), options)? {
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
