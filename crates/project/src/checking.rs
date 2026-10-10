use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    sync::{Arc, atomic::Ordering},
};

use instar_luau::{AnalysisPurpose, AnalysisResult};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, Event, EventPayload, Options, Severity,
    SourceOutcome, SourceVersion,
};

use instar_syntax::{DirectiveKind, directives, parse};

use crate::{
    cache::AnalysisEntry,
    capture::Capture,
    configuration::{DiagnosticPolicy, OperationOverrides, worker_options},
    execution::{Runtime, Work, WorkOutput, failure},
    graph::{ClosureRecipe, DependencyGraph},
    inputs::ProjectSnapshot,
    report::{CheckReport, Counts, ModuleDiagnostic},
    request::CheckRequest,
    selection::{SelectionPurpose, SelectionResult},
};

struct CheckState {
    completion: Completion,
    findings: Vec<Diagnostic>,
    module_findings: Vec<ModuleDiagnostic>,
    deferred: Option<io::Error>,
    outcomes: BTreeMap<SourceVersion, Completion>,
}

impl Runtime {
    pub(crate) fn check(
        &mut self,
        request: &CheckRequest,
        snapshot: Option<ProjectSnapshot>,
        options: &Options,
    ) -> io::Result<CheckReport> {
        let mut capture = snapshot.map_or_else(
            || Capture::live(Arc::clone(&self.host)),
            |snapshot| Capture::new(Arc::clone(&self.host), snapshot),
        );

        let overrides = OperationOverrides::Check(request.overrides.clone());

        let selected = self.select(
            &request.invocation,
            &overrides,
            &mut capture,
            SelectionPurpose::Command,
            options,
        )?;

        let graph = self.graph(
            &selected.targets,
            &request.invocation,
            &overrides,
            &mut capture,
            true,
            options,
        )?;

        let (recipes, completion, mut findings) =
            self.closures(&graph, AnalysisPurpose::Source, &[], &capture, options)?;

        let mut module_findings = graph.module_findings.clone();

        for diagnostic in &findings {
            module_findings.extend(
                graph
                    .modules
                    .keys()
                    .filter(|module| module.source == diagnostic.primary.source.identity)
                    .map(|module| ModuleDiagnostic {
                        module: *module,
                        diagnostic: diagnostic.clone(),
                    }),
            );
        }

        findings.extend(graph.diagnostics.clone());

        let deferred = graph
            .modules
            .values()
            .flat_map(|node| node.environment.failures.iter())
            .next()
            .cloned()
            .map(io::Error::from);

        let mut state = CheckState {
            completion,
            findings,
            module_findings,
            deferred,
            outcomes: BTreeMap::new(),
        };

        let results = self.analyze(&recipes, options);
        collect_analysis(results, &graph, &mut state, options);

        self.check_report(request, selected, graph, capture, state, options)
    }

    fn check_report(
        &self,
        request: &CheckRequest,
        selected: SelectionResult,
        graph: DependencyGraph,
        capture: Capture,
        state: CheckState,
        options: &Options,
    ) -> io::Result<CheckReport> {
        let CheckState {
            completion,
            findings,
            mut module_findings,
            deferred,
            ..
        } = state;

        let environment_diagnostics = findings
            .iter()
            .filter(|finding| finding.category == DiagnosticCategory::Configuration)
            .cloned()
            .collect();

        let accumulated = findings.clone();

        let (findings, failed) = match reporting(
            findings,
            &capture.snapshot,
            &request.overrides.diagnostics,
            false,
            options,
        ) {
            Ok(reported) => reported,

            Err(error) => {
                let mut failure = failure(&deferred.unwrap_or(error), options, &self.events);
                failure.completion = failure.completion.combine(completion);

                for diagnostic in accumulated {
                    if !failure.diagnostics.contains(&diagnostic) {
                        failure.diagnostics.push(diagnostic);
                    }
                }

                return Err(failure.into());
            }
        };

        if let Some(error) = deferred {
            let mut failure = failure(&error, options, &self.events);
            failure.completion = failure.completion.combine(completion);

            for diagnostic in findings {
                if !failure.diagnostics.contains(&diagnostic) {
                    failure.diagnostics.push(diagnostic);
                }
            }

            return Err(failure.into());
        }

        module_findings.retain_mut(|finding| {
            if let Some(reported) = findings
                .iter()
                .find(|diagnostic| diagnostic.identity == finding.diagnostic.identity)
            {
                finding.diagnostic = reported.clone();

                true
            } else {
                false
            }
        });

        module_findings.sort_by(|left, right| {
            left.module
                .cmp(&right.module)
                .then_with(|| left.diagnostic.identity.cmp(&right.diagnostic.identity))
        });

        module_findings.dedup_by(|left, right| {
            left.module == right.module && left.diagnostic == right.diagnostic
        });

        let sources = graph
            .modules
            .values()
            .map(|node| node.source.version)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let modules = graph.modules.keys().copied().collect();

        let counts = Counts {
            selected: graph.roots.len(),
            dependencies: graph.modules.len().saturating_sub(graph.roots.len()),
            excluded: selected
                .decisions
                .iter()
                .filter(|decision| !decision.selected)
                .count(),
            findings: findings.len(),
            ..Counts::default()
        };

        Ok(CheckReport {
            snapshot: capture.snapshot,
            inputs: capture.inputs.into_values().collect(),
            sources,
            modules,
            module_findings,
            imports: graph.imports,
            findings,
            environment_diagnostics,
            selection: selected.decisions,
            dependencies: graph.edges,
            settings: graph
                .modules
                .into_values()
                .map(|node| node.effective)
                .collect(),
            counts,
            completion,
            failed,
        })
    }

    pub(crate) fn analyze(
        &mut self,
        recipes: &[ClosureRecipe],
        options: &Options,
    ) -> Vec<io::Result<Arc<AnalysisResult>>> {
        let epoch = self.host.worker_epoch.load(Ordering::Acquire);

        if epoch != self.epoch {
            if let Err(error) = self.workers.discard() {
                return vec![Err(error)];
            }

            self.epoch = epoch;
        }

        let current = self.host.snapshot();

        self.cache
            .analyses
            .retain(|entry| current.current(&entry.inputs));

        let mut work = Vec::new();
        let mut indices = Vec::new();
        let mut results = (0..recipes.len()).map(|_| None).collect::<Vec<_>>();

        for (index, recipe) in recipes.iter().enumerate() {
            if let Some(entry) = self.cache.analyses.iter().find(|entry| {
                equivalent(&entry.recipe, &recipe.request)
                    && entry.result.completion == Completion::Complete
            }) {
                results[index] = Some(Ok(Arc::clone(&entry.result)));
            } else {
                indices.push(index);
                work.push(Work::Analysis(recipe.request.clone()));
            }
        }

        for (index, result) in indices
            .into_iter()
            .zip(self.workers.batch(work, &worker_options(options)))
        {
            match result {
                Ok(WorkOutput::Analysis(result)) => {
                    let result = Arc::new(result);

                    if result.completion == Completion::Complete {
                        self.cache.analyses.push(AnalysisEntry {
                            recipe: recipes[index].request.clone(),
                            result: Arc::clone(&result),
                            inputs: recipes[index].inputs.clone(),
                        });
                    }

                    results[index] = Some(Ok(result));
                }

                Ok(_) => {
                    results[index] = Some(Err(io::Error::other(
                        "analysis lane returned wrong outcome",
                    )));
                }

                Err(error) => {
                    results[index] = Some(Err(error));
                }
            }
        }

        results
            .into_iter()
            .map(|result| {
                result.unwrap_or_else(|| Err(io::Error::other("analysis produced no outcome")))
            })
            .collect()
    }
}

fn collect_analysis(
    results: Vec<io::Result<Arc<AnalysisResult>>>,
    graph: &DependencyGraph,
    state: &mut CheckState,
    options: &Options,
) {
    for result in results {
        match result {
            Ok(result) => {
                state.completion = state.completion.clone().combine(result.completion.clone());

                for module in &result.modules {
                    let identity = module.analysis.module;

                    let covered = module
                        .completion
                        .clone()
                        .combine(graph.modules[&identity].environment.completion.clone());

                    state.completion = state.completion.clone().combine(covered.clone());

                    for diagnostic in module
                        .parsed
                        .diagnostics
                        .iter()
                        .chain(&module.diagnostics)
                        .chain(
                            module
                                .environment
                                .iter()
                                .flat_map(|environment| &environment.diagnostics),
                        )
                    {
                        if diagnostic.category == DiagnosticCategory::Syntax
                            && graph.diagnostics.iter().any(|finding| {
                                finding.category == DiagnosticCategory::Syntax
                                    && finding.primary == diagnostic.primary
                            })
                        {
                            continue;
                        }

                        state.findings.push(diagnostic.clone());

                        state.module_findings.push(ModuleDiagnostic {
                            module: identity,
                            diagnostic: diagnostic.clone(),
                        });
                    }

                    state
                        .outcomes
                        .entry(module.parsed.input.source.version)
                        .and_modify(|previous: &mut Completion| {
                            *previous = previous.clone().combine(covered.clone());
                        })
                        .or_insert(covered);
                }
            }

            Err(error) => {
                if state.deferred.is_none() {
                    state.deferred = Some(error);
                }
            }
        }
    }

    for (source, covered) in mem::take(&mut state.outcomes) {
        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::SourceOutcome(SourceOutcome {
                source,
                completion: covered,
            }),
        });
    }
}

pub(crate) fn equivalent(
    left: &instar_luau::AnalysisRequest,
    right: &instar_luau::AnalysisRequest,
) -> bool {
    fn normalized(request: &instar_luau::AnalysisRequest) -> instar_luau::AnalysisRequest {
        let mut request = request.clone();
        request.closure = instar_luau::ClosureIdentity::new(0);

        for module in &mut request.modules {
            module.analysis.revision = 0;

            if let Some(environment) = &mut module.environment {
                environment.identity = instar_luau::EnvironmentIdentity::new(0);
            }
        }

        request
    }

    normalized(left) == normalized(right)
}

pub(crate) fn reporting(
    mut findings: Vec<Diagnostic>,
    snapshot: &ProjectSnapshot,
    policy: &DiagnosticPolicy,
    lint: bool,
    options: &Options,
) -> io::Result<(Vec<Diagnostic>, bool)> {
    findings.sort_by(|left, right| {
        left.primary
            .source
            .cmp(&right.primary.source)
            .then_with(|| left.primary.span.start.cmp(&right.primary.span.start))
            .then_with(|| left.code.cmp(&right.code))
    });

    findings.dedup_by(|left, right| {
        left.code == right.code && left.primary == right.primary && left.message == right.message
    });

    let mut retained = Vec::new();
    let mut failed = false;

    for finding in findings {
        options.check()?;

        let suppressed = if lint {
            snapshot.source(finding.primary.source.identity).is_some_and(|source| {
                let syntax = parse(source, &instar_syntax::SyntaxSettings::default(), options);
                let directives = directives(&syntax.tree, options);

                directives.directives.iter().any(|directive| matches!(&directive.kind, DirectiveKind::Lint(names) if (names.is_empty() || names.contains(&finding.code)) && directive.scope.start <= finding.primary.span.start && finding.primary.span.end <= directive.scope.end))
            })
        } else {
            false
        };

        if suppressed {
            continue;
        }

        failed |= rank(finding.severity) <= rank(policy.threshold)
            || (policy.warnings_as_errors && finding.severity == Severity::Warning);

        if policy
            .codes
            .as_ref()
            .is_some_and(|codes| !codes.contains(&finding.code))
            || policy
                .categories
                .as_ref()
                .is_some_and(|categories| !categories.contains(&finding.category))
            || policy
                .severities
                .as_ref()
                .is_some_and(|severities| !severities.contains(&finding.severity))
        {
            continue;
        }

        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::Diagnostic(finding.clone()),
        });

        retained.push(finding);
    }

    Ok((retained, failed))
}

fn rank(severity: Severity) -> u8 {
    match severity {
        Severity::Error => 0,
        Severity::Warning => 1,
        Severity::Information => 2,
        Severity::Hint => 3,
    }
}
