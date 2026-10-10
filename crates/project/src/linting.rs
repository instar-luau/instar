use std::{
    collections::{BTreeMap, BTreeSet},
    io, iter, mem,
    sync::Arc,
};

use instar_lint::{
    self as lint, FixValidation, LintSettings, ResolvedDependency, RuleAvailability, RuleFix,
    RuleIdentity, RuleInput, RuleProducer,
};

use instar_luau::{
    AnalysisPurpose, FactsResult, ImportOutcome, ImportRequest, LanguageMode,
    LintResult as NativeLintResult, ModuleAnalysis, ModuleIdentity, ResolvedImport,
    SemanticCategory, UpstreamFinding, UpstreamWarningSettings, WarningAvailability,
};

use instar_shared::{
    Availability, Completion, Diagnostic, DiagnosticCategory, DiagnosticTag, EditIdentity,
    EditIntent, EditProposal, Event, EventPayload, FindingIdentity, Knowledge, Location,
    OperationFailure, Options, Progress, Reason, Severity, SourceIdentity, SourceOutcome,
    SourceVersion, Span,
};

use instar_syntax::{Directive, DirectiveKind, bindings, directives, parse};

use crate::{
    capture::Capture,
    change::{ChangeBatch, ProjectChange, SourceChange},
    checking,
    configuration::{
        CheckOverrides, DiagnosticPolicy, EffectiveSettings, OperationOverrides, worker_options,
    },
    editing::{
        ApplyRequest, EditGuarantee, ValidatedEdit, WriteAuthorization, WriteOutcome, preview,
    },
    environments,
    execution::{Runtime, Work, WorkOutput},
    graph::{ClosureRecipe, DependencyGraph, ModuleNode, syntax_settings},
    inputs::{ProjectSnapshot, SourceOrigin},
    report::{Counts, DiagnosticRecord, LintReport, ModuleDiagnostic},
    request::{CheckRequest, FixMode, Input, Invocation, LintRequest},
    selection::{SelectionPurpose, SelectionResult},
};

struct LintContext {
    capture: Capture,
    selected: SelectionResult,
    graph: DependencyGraph,
    ordinary: BTreeSet<SourceIdentity>,
    categories: Vec<SemanticCategory>,
    requested_native: bool,
    failures: Vec<OperationFailure>,
}

struct LintState {
    findings: Vec<DiagnosticRecord>,
    fixes: Vec<RuleFix>,
    availability: Vec<RuleAvailability>,
    suppressions: BTreeMap<SourceVersion, Vec<Directive>>,
    suppression_findings: Vec<Diagnostic>,
    emitted: BTreeSet<FindingIdentity>,
    environment: BTreeMap<ModuleIdentity, Availability>,
    prerequisites: BTreeSet<FindingIdentity>,
    prepared: BTreeSet<ModuleIdentity>,
    producer_completion: BTreeMap<ModuleIdentity, Completion>,
    syntax_completion: BTreeMap<ModuleIdentity, Completion>,
    outcomes: Vec<SourceOutcome>,
    native_syntax: BTreeSet<SourceVersion>,
    completed: Completion,
    facts: BTreeMap<ModuleIdentity, FactsResult>,
    warnings: BTreeSet<(ModuleIdentity, RuleIdentity)>,
}

impl LintState {
    fn new(context: &LintContext) -> Self {
        Self {
            findings: context.graph.findings.clone(),
            fixes: Vec::new(),
            availability: Vec::new(),
            suppressions: BTreeMap::new(),
            suppression_findings: Vec::new(),
            emitted: BTreeSet::new(),
            environment: context
                .graph
                .modules
                .values()
                .map(|node| {
                    (
                        node.identity,
                        if node.environment.requested {
                            coverage(&node.environment.completion, Reason::UnavailableEnvironment)
                        } else {
                            Availability::Unavailable(Reason::UnavailableEnvironment)
                        },
                    )
                })
                .collect(),
            prerequisites: context
                .graph
                .modules
                .values()
                .flat_map(|node| {
                    node.environment
                        .diagnostics
                        .iter()
                        .map(|finding| finding.identity.clone())
                })
                .collect(),
            prepared: BTreeSet::new(),
            producer_completion: BTreeMap::new(),
            syntax_completion: BTreeMap::new(),
            outcomes: Vec::new(),
            native_syntax: BTreeSet::new(),
            completed: context
                .selected
                .completion
                .clone()
                .combine(context.graph.completion.clone()),
            facts: BTreeMap::new(),
            warnings: BTreeSet::new(),
        }
    }
}

struct FixApplication {
    snapshot: ProjectSnapshot,
    accepted: BTreeSet<SourceIdentity>,
}

impl Runtime {
    pub(crate) fn lint(
        &mut self,
        request: &LintRequest,
        snapshot: Option<ProjectSnapshot>,
        options: &Options,
    ) -> io::Result<LintReport> {
        let mut context = self.lint_context(request, snapshot, options)?;
        let mut state = LintState::new(&context);
        lint_syntax(&context, &mut state, options);
        stream_early_findings(request, &context, &mut state, options);
        let recipes = self.lint_recipes(&context, &mut state, options)?;

        if context.requested_native && options.interrupted().is_none() {
            self.lint_analysis(&recipes, &context, &mut state, options);
            fold_lint_environment(&recipes, &mut state);
            self.lint_prerequisites(&recipes, &context, &mut state, options)?;
        }

        lint_modules(&context, &mut state, options);
        stream_lint_outcomes(&context, &mut state, options);
        filter_lint_findings(&context, &mut state);
        self.prepare_lint_fixes(&context, &mut state);
        let mut report = lint_report(request, &mut context, &mut state, options);
        self.finish_lint_fixes(request, &context, &mut state, &mut report, options);

        if let Some(interrupted) = options.interrupted() {
            report.completion = report.completion.combine(interrupted);
        }

        report.counts.findings = report.final_findings.len();

        finish_lint_failures(context, report)
    }

    fn lint_fixes(
        &mut self,
        request: &LintRequest,
        targets: &BTreeSet<SourceIdentity>,
        modules: &BTreeSet<ModuleIdentity>,
        report: &mut LintReport,
        options: &Options,
    ) -> io::Result<()> {
        let fixes = collect_lint_fixes(request, targets, report, options)?;

        if fixes.is_empty() {
            return Ok(());
        }

        let validated = self.validate_lint_fixes(request, &fixes, report, options)?;

        let changed = validated
            .proposal()
            .edits
            .iter()
            .map(|edit| edit.location.source.identity)
            .collect::<BTreeSet<_>>();

        report.counts.changed = changed.len();
        report.proposals.push(validated.clone());
        report.snapshots.push(validated.original().clone());
        report.inputs = validated.inputs().to_vec();

        if matches!(request.fixes, FixMode::Preview) {
            return preview_lint_fixes(&validated, &changed, report, options);
        }

        let mut application = if matches!(request.fixes, FixMode::Write) {
            self.write_lint_fixes(&validated, &changed, report, options)?
        } else {
            FixApplication {
                snapshot: report.snapshot.clone(),
                accepted: BTreeSet::new(),
            }
        };

        accept_stdin_fixes(&validated, &changed, report, &mut application, options)?;
        report.snapshot = application.snapshot.clone();

        report.counts.fixes = fixes
            .iter()
            .filter(|fix| {
                fix.proposal.edits.iter().all(|edit| {
                    application
                        .accepted
                        .contains(&edit.location.source.identity)
                })
            })
            .count();

        if application.accepted.is_empty() {
            return Ok(());
        }

        self.refresh_lint_fixes(request, modules, report, application, options)
    }

    fn lint_context(
        &mut self,
        request: &LintRequest,
        snapshot: Option<ProjectSnapshot>,
        options: &Options,
    ) -> io::Result<LintContext> {
        let mut capture = snapshot.map_or_else(
            || Capture::live(Arc::clone(&self.host)),
            |snapshot| Capture::new(Arc::clone(&self.host), snapshot),
        );

        let overrides = OperationOverrides::Lint(request.overrides.clone());

        let selected = self.select(
            &request.invocation,
            &overrides,
            &mut capture,
            SelectionPurpose::Command,
            options,
        )?;

        let stdin = selected
            .decisions
            .iter()
            .filter_map(|decision| decision.source)
            .filter(|identity| {
                matches!(
                    capture.snapshot.origin(*identity),
                    Some(SourceOrigin::Stdin { .. })
                )
            })
            .collect::<BTreeSet<_>>()
            .len();

        if matches!(request.fixes, FixMode::Stdin) && stdin != 1
            || matches!(request.fixes, FixMode::Write) && stdin > 1
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "stdin fixing requires exactly one emitted stdin source",
            ));
        }

        let prepare_environment = selected.targets.iter().any(|target| {
            target
                .effective
                .settings
                .linting
                .enabled()
                .any(|(rule, _)| {
                    rule.prerequisites.environment || !rule.prerequisites.semantics.is_empty()
                })
        });

        let graph = self.graph(
            &selected.targets,
            &request.invocation,
            &overrides,
            &mut capture,
            prepare_environment,
            options,
        )?;

        let failures = graph
            .modules
            .values()
            .flat_map(|node| node.environment.failures.iter().cloned())
            .collect();

        let ordinary = graph
            .modules
            .values()
            .map(|node| node.source.version.identity)
            .collect::<BTreeSet<_>>();

        let categories = graph
            .modules
            .values()
            .flat_map(|node| {
                node.effective
                    .settings
                    .linting
                    .enabled()
                    .flat_map(|(rule, _)| rule.prerequisites.semantics.iter().copied())
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let requested_native = !categories.is_empty()
            || graph.modules.values().any(|node| {
                node.effective.settings.linting.enabled().any(|(rule, _)| {
                    rule.prerequisites.environment
                        || matches!(rule.producer, RuleProducer::Upstream(_))
                })
            });

        Ok(LintContext {
            capture,
            selected,
            graph,
            ordinary,
            categories,
            requested_native,
            failures,
        })
    }

    fn lint_recipes(
        &mut self,
        context: &LintContext,
        state: &mut LintState,
        options: &Options,
    ) -> io::Result<Vec<ClosureRecipe>> {
        let (mut recipes, completion, closure_diagnostics) = if context.requested_native {
            match self.closures(
                &context.graph,
                AnalysisPurpose::Source,
                &context.categories,
                &context.capture,
                options,
            ) {
                Ok(result) => result,

                Err(error) => {
                    if let Some(interrupted) = options.interrupted() {
                        (Vec::new(), interrupted, Vec::new())
                    } else {
                        return Err(error);
                    }
                }
            }
        } else {
            (Vec::new(), Completion::Complete, Vec::new())
        };

        state.completed = state.completed.clone().combine(completion);

        state.findings.extend(closure_diagnostics);

        for recipe in &mut recipes {
            let required = recipe.request.modules.iter().any(|module| {
                context.graph.modules[&module.analysis.module]
                    .effective
                    .settings
                    .linting
                    .enabled()
                    .any(|(rule, _)| !rule.prerequisites.semantics.is_empty())
            });

            if required
                && recipe.request.modules.iter().any(|module| {
                    !context.graph.modules[&module.analysis.module]
                        .environment
                        .requested
                })
            {
                for module in &recipe.request.modules {
                    state.environment.insert(
                        module.analysis.module,
                        Availability::Unavailable(Reason::UnavailableEnvironment),
                    );
                }
            }

            for module in &mut recipe.request.modules {
                module.reporting.ordinary_type_diagnostics = false;
                let node = &context.graph.modules[&module.analysis.module];

                let available = node.environment.requested
                    && node.environment.completion == Completion::Complete;

                module.inference.required = required && available;

                module.inference.retained = if module.inference.required {
                    node.effective
                        .settings
                        .linting
                        .enabled()
                        .flat_map(|(rule, _)| rule.prerequisites.semantics.iter().copied())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };

                module.environment = (available
                    && (required
                        || node
                            .effective
                            .settings
                            .linting
                            .enabled()
                            .any(|(rule, _)| rule.prerequisites.environment)))
                .then(|| node.environment.input.clone());
            }
        }

        Ok(recipes)
    }

    fn lint_analysis(
        &mut self,
        recipes: &[ClosureRecipe],
        context: &LintContext,
        state: &mut LintState,
        options: &Options,
    ) {
        for (recipe, result) in recipes.iter().zip(self.analyze(recipes, options)) {
            match result {
                Ok(result) => {
                    state.completed = state.completed.clone().combine(result.completion.clone());

                    for module in &result.modules {
                        record_lint_analysis(module, context, state);
                    }
                }

                Err(error) => {
                    if let Some(interrupted) = options.interrupted() {
                        state.completed = state.completed.clone().combine(interrupted);
                        break;
                    }

                    state.completed = state
                        .completed
                        .clone()
                        .combine(Completion::Incomplete(vec![Reason::FailedPrerequisite]));

                    for module in &recipe.request.modules {
                        accumulate(
                            &mut state.producer_completion,
                            module.analysis.module,
                            Completion::Incomplete(vec![Reason::FailedPrerequisite]),
                        );

                        let finding = diagnostic(
                            module.source.source.version,
                            "analysis.failure",
                            error.to_string(),
                            DiagnosticCategory::Analysis,
                            Severity::Error,
                            Span { start: 0, end: 0 },
                        );

                        state.findings.push(DiagnosticRecord {
                            module: Some(module.analysis.module),
                            diagnostic: finding,
                        });
                    }
                }
            }
        }
    }

    fn lint_prerequisites(
        &mut self,
        recipes: &[ClosureRecipe],
        context: &LintContext,
        state: &mut LintState,
        options: &Options,
    ) -> io::Result<()> {
        let (work, identities) = lint_work(recipes, context, state, options);

        for (identity, result) in identities
            .into_iter()
            .zip(self.workers.batch(work, &worker_options(options)))
        {
            match result {
                Ok(WorkOutput::Facts(mut result)) => {
                    if result.analysis.module != identity {
                        return Err(io::Error::other(
                            "lint facts identify a different captured module",
                        ));
                    }

                    result.diagnostics = result
                        .diagnostics
                        .into_iter()
                        .filter(|diagnostic| {
                            !checking::duplicate_resolution(diagnostic, &context.graph, identity)
                        })
                        .map(|diagnostic| {
                            checking::prerequisite_resolution(&diagnostic, &context.graph, identity)
                        })
                        .collect();

                    state.completed = state.completed.clone().combine(result.completion.clone());

                    accumulate(
                        &mut state.producer_completion,
                        identity,
                        result.completion.clone(),
                    );

                    state
                        .findings
                        .extend(result.diagnostics.iter().cloned().map(|diagnostic| {
                            DiagnosticRecord {
                                module: Some(identity),
                                diagnostic,
                            }
                        }));

                    state.facts.insert(identity, result);
                }

                Ok(WorkOutput::Warnings(result)) => {
                    record_native_warnings(result, identity, context, state)?;
                }

                Ok(_) => {
                    return Err(io::Error::other(
                        "lint prerequisite lane returned wrong outcome",
                    ));
                }

                Err(error) => {
                    if let Some(interrupted) = options.interrupted() {
                        state.completed = state.completed.clone().combine(interrupted);
                        break;
                    }

                    state.completed = state
                        .completed
                        .clone()
                        .combine(Completion::Incomplete(vec![Reason::FailedPrerequisite]));

                    accumulate(
                        &mut state.producer_completion,
                        identity,
                        Completion::Incomplete(vec![Reason::FailedPrerequisite]),
                    );

                    let finding = diagnostic(
                        context.graph.modules[&identity].source.version,
                        "analysis.failure",
                        error.to_string(),
                        DiagnosticCategory::Analysis,
                        Severity::Error,
                        Span { start: 0, end: 0 },
                    );

                    state.findings.push(DiagnosticRecord {
                        module: Some(identity),
                        diagnostic: finding,
                    });
                }
            }
        }

        Ok(())
    }

    fn prepare_lint_fixes(&mut self, context: &LintContext, state: &mut LintState) {
        state.fixes.retain(|fix| {
            fix.proposal.edits.iter().all(|edit| {
                context
                    .selected
                    .targets
                    .iter()
                    .any(|target| edit.location.source.identity == target.identity)
            }) && fix.proposal.edits.iter().all(|edit| {
                state
                    .findings
                    .iter()
                    .map(|finding| &finding.diagnostic)
                    .any(|finding| {
                        finding.code == fix.rule.name()
                            && finding.primary.source == edit.location.source
                            && (finding.primary.span.start <= edit.location.span.start
                                && edit.location.span.end <= finding.primary.span.end
                                || edit.location.span.start <= finding.primary.span.start
                                    && finding.primary.span.end <= edit.location.span.end)
                            && !suppressed(finding, &state.suppressions, &context.capture.snapshot)
                    })
            })
        });

        state.fixes.sort_by(|left, right| {
            left.rule.cmp(&right.rule).then_with(|| {
                left.proposal
                    .edits
                    .first()
                    .map(|edit| (edit.location.source, edit.location.span.start))
                    .cmp(
                        &right
                            .proposal
                            .edits
                            .first()
                            .map(|edit| (edit.location.source, edit.location.span.start)),
                    )
            })
        });

        state.fixes.dedup_by(|left, right| {
            left.rule == right.rule
                && left.proposal.edits == right.proposal.edits
                && left.validation == right.validation
        });

        for fix in &mut state.fixes {
            fix.proposal.identity = EditIdentity::new(self.identity());
        }
    }

    fn finish_lint_fixes(
        &mut self,
        request: &LintRequest,
        context: &LintContext,
        state: &mut LintState,
        report: &mut LintReport,
        options: &Options,
    ) {
        if !matches!(request.fixes, FixMode::None)
            && report.completion == Completion::Complete
            && !report.fixes.is_empty()
            && let Err(error) = self.lint_fixes(
                request,
                &context
                    .selected
                    .targets
                    .iter()
                    .map(|target| target.identity)
                    .collect(),
                &context.graph.roots,
                report,
                options,
            )
        {
            record_fix_failure(request, context, state, report, &error, options);
        }
    }

    fn validate_lint_fixes(
        &mut self,
        request: &LintRequest,
        fixes: &[RuleFix],
        report: &LintReport,
        options: &Options,
    ) -> io::Result<ValidatedEdit> {
        let proposal = EditProposal {
            identity: EditIdentity::new(self.identity()),
            edits: fixes
                .iter()
                .flat_map(|fix| fix.proposal.edits.clone())
                .collect(),
            guards: fixes
                .iter()
                .flat_map(|fix| fix.proposal.guards.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            intent: EditIntent::Fix,
        };

        let mut guarantees = vec![
            EditGuarantee::Ranges,
            EditGuarantee::Syntax,
            EditGuarantee::Diagnostics,
            EditGuarantee::Resolution,
        ];

        if fixes
            .iter()
            .any(|fix| matches!(fix.validation, FixValidation::Bindings))
        {
            guarantees.push(EditGuarantee::Bindings);
        }

        let validation = CheckRequest {
            invocation: request.invocation.clone(),
            overrides: CheckOverrides {
                selection: request.overrides.selection.clone(),
                language: request.overrides.language.clone(),
                environment: request.overrides.environment.clone(),
                resolution: request.overrides.resolution.clone(),
                ..CheckOverrides::default()
            },
        };

        let validated = self.validate_edit(
            proposal,
            &report.snapshot,
            &report.inputs,
            guarantees,
            &validation,
            options,
        )?;

        Ok(validated)
    }

    fn write_lint_fixes(
        &mut self,
        validated: &ValidatedEdit,
        changed: &BTreeSet<SourceIdentity>,
        report: &mut LintReport,
        options: &Options,
    ) -> io::Result<FixApplication> {
        let mut application = FixApplication {
            snapshot: report.snapshot.clone(),
            accepted: BTreeSet::new(),
        };

        let destinations = validated
            .proposal()
            .guards
            .iter()
            .filter(|version| changed.contains(&version.identity))
            .filter_map(|version| match report.snapshot.origin(version.identity) {
                Some(SourceOrigin::Disk(path)) => Some(WriteAuthorization {
                    source: *version,
                    destination: path.clone(),
                }),

                _ => None,
            })
            .collect();

        let applied = self.apply(
            ApplyRequest {
                edit: validated.clone(),
                destinations,
            },
            options,
        )?;

        report.completion = report.completion.clone().combine(applied.completion);

        for outcome in &applied.writes {
            match outcome {
                WriteOutcome::Written { source, revision } => {
                    let current = applied.snapshot.source(source.identity).ok_or_else(|| {
                        io::Error::other("written source is absent from accepted snapshot")
                    })?;

                    let proposed =
                        validated
                            .proposed()
                            .source(source.identity)
                            .ok_or_else(|| {
                                io::Error::other("written source is absent from validated snapshot")
                            })?;

                    if current.version.revision != *revision || current.text != proposed.text {
                        return Err(io::Error::other(
                            "writer did not publish the accepted source revision",
                        ));
                    }

                    application.accepted.insert(source.identity);
                    report.counts.written += 1;
                }

                WriteOutcome::Failed { .. } | WriteOutcome::Unapplied { .. } => {
                    report.counts.unsuccessful += 1;
                }

                WriteOutcome::NotRequested | WriteOutcome::Unchanged => {}
            }
        }

        if !application.accepted.is_empty() {
            application.snapshot = applied.snapshot;
        }

        report.writes = applied.writes;

        for identity in changed.iter().filter(|identity| {
            !matches!(
                report.snapshot.origin(**identity),
                Some(SourceOrigin::Disk(_) | SourceOrigin::Stdin { .. })
            )
        }) {
            let source = report
                .snapshot
                .source(*identity)
                .ok_or_else(|| io::Error::other("unwritable fix source is absent"))?;

            report.writes.push(WriteOutcome::Unapplied {
                source: source.version,
                reason: "synchronized and virtual sources require caller-managed edit application"
                    .to_owned(),
            });

            report.counts.unsuccessful += 1;

            report.completion = report
                .completion
                .clone()
                .combine(Completion::Incomplete(vec![Reason::UnsupportedFeature]));
        }

        Ok(application)
    }

    fn refresh_lint_fixes(
        &mut self,
        request: &LintRequest,
        modules: &BTreeSet<ModuleIdentity>,
        report: &mut LintReport,
        application: FixApplication,
        options: &Options,
    ) -> io::Result<()> {
        report.snapshots.push(application.snapshot.clone());

        let invocation = Invocation {
            inputs: modules.iter().copied().map(Input::Module).collect(),
            ..request.invocation.clone()
        };

        let mut final_options = options.child();
        final_options.observer = None;

        let final_report = self.lint(
            &LintRequest {
                invocation,
                fixes: FixMode::None,
                ..request.clone()
            },
            Some(application.snapshot),
            &final_options,
        )?;

        report.snapshot = final_report.snapshot;
        report.snapshots.extend(final_report.snapshots);
        report.modules = final_report.modules;
        report.module_findings = final_report.module_findings;
        report.imports = final_report.imports;

        report.outcomes.retain(|outcome| {
            !final_report
                .outcomes
                .iter()
                .any(|final_outcome| final_outcome.source == outcome.source)
        });

        for outcome in &final_report.outcomes {
            options.report(Event {
                invocation: 0,
                operation: 0,
                payload: EventPayload::SourceOutcome(outcome.clone()),
            });
        }

        report.outcomes.extend(final_report.outcomes);
        report.inputs = final_report.inputs;
        report.sources.extend(final_report.sources);
        report.sources.sort();
        report.sources.dedup();
        report.availability = final_report.availability;
        report.final_findings = final_report.findings;
        report.failed = final_report.failed;
        report.completion = report.completion.clone().combine(final_report.completion);
        report.dependencies = final_report.dependencies;
        report.settings = final_report.settings;

        Ok(())
    }
}

fn lint_syntax(context: &LintContext, state: &mut LintState, options: &Options) {
    progress(options, 0, context.graph.modules.len());

    for (index, node) in context.graph.modules.values().enumerate() {
        if let Some(interrupted) = options.interrupted() {
            state.completed = state.completed.clone().combine(interrupted);
            break;
        }

        let syntax = parse(&node.source, &syntax_settings(&node.effective), options);
        let bindings = bindings(&syntax.tree, options);
        let directives = directives(&syntax.tree, options);

        state
            .suppressions
            .insert(node.source.version, directives.directives.clone());

        if !context.graph.roots.contains(&node.identity) {
            progress(options, index + 1, context.graph.modules.len());
            continue;
        }

        let imports = dependencies(&node.imports);
        let mut settings = node.effective.settings.linting.clone();

        for rule in lint::rules() {
            if (rule.prerequisites.environment || !rule.prerequisites.semantics.is_empty())
                && let Some(configuration) = settings.rules.get_mut(&rule.identity)
            {
                configuration.enabled = false;
            }
        }

        let result = lint::run(
            &RuleInput {
                source: &node.source,
                syntax: &syntax.tree,
                bindings: &bindings,
                facts: None,
                imports: &imports,
                settings: &settings,
                directives: &directives,
            },
            options,
        );

        state
            .syntax_completion
            .insert(node.identity, result.completion.clone());

        state.findings.extend(
            result
                .findings
                .iter()
                .cloned()
                .map(|diagnostic| DiagnosticRecord {
                    module: Some(node.identity),
                    diagnostic,
                }),
        );

        state.completed = state.completed.clone().combine(result.completion);
        state.fixes.extend(result.fixes);
        progress(options, index + 1, context.graph.modules.len());
    }
}

fn stream_early_findings(
    request: &LintRequest,
    context: &LintContext,
    state: &mut LintState,
    options: &Options,
) {
    let early = state
        .findings
        .iter()
        .map(|finding| &finding.diagnostic)
        .filter(|finding| retained(finding, &context.ordinary, &state.prerequisites))
        .filter(|finding| {
            !context.requested_native
                || finding.category != DiagnosticCategory::Syntax
                || finding.code != "syntax.parse"
        })
        .filter(|finding| {
            finding.code != "unused_variable"
                || !context.graph.modules.values().any(|node| {
                    node.source.version == finding.primary.source
                        && node.effective.settings.linting.upstream().any(|warning| {
                            matches!(
                                warning.name.as_str(),
                                "LocalUnused" | "FunctionUnused" | "ImportUnused"
                            ) && precedence(&node.effective, &warning.name)
                                >= precedence(&node.effective, &finding.code)
                        })
                })
        })
        .map(|diagnostic| DiagnosticRecord {
            module: None,
            diagnostic: diagnostic.clone(),
        })
        .collect();

    drop(reporting(
        prepared(early, &state.suppressions, &context.capture.snapshot),
        &context.graph,
        &request.overrides.diagnostics,
        &mut state.emitted,
        options,
    ));
}

fn record_lint_analysis(module: &ModuleAnalysis, context: &LintContext, state: &mut LintState) {
    state.prepared.insert(module.analysis.module);

    if module.parsed.completion == Completion::Complete {
        state
            .native_syntax
            .insert(module.parsed.input.source.version);
    }

    accumulate(
        &mut state.producer_completion,
        module.analysis.module,
        module.completion.clone(),
    );

    state.findings.extend(
        module
            .parsed
            .diagnostics
            .iter()
            .chain(&module.diagnostics)
            .filter(|diagnostic| {
                !checking::duplicate_resolution(diagnostic, &context.graph, module.analysis.module)
            })
            .map(|diagnostic| {
                checking::prerequisite_resolution(
                    diagnostic,
                    &context.graph,
                    module.analysis.module,
                )
            })
            .map(|diagnostic| DiagnosticRecord {
                module: Some(module.analysis.module),
                diagnostic,
            }),
    );

    state.completed = state.completed.clone().combine(module.completion.clone());

    if let Some(metadata) = &module.environment
        && context.graph.modules[&module.analysis.module]
            .environment
            .requested
        && context.graph.modules[&module.analysis.module]
            .environment
            .completion
            == Completion::Complete
    {
        let status = combine(
            state
                .environment
                .get(&module.analysis.module)
                .copied()
                .unwrap_or(Availability::Unavailable(Reason::UnavailableEnvironment)),
            combine(
                metadata.availability,
                coverage(&metadata.completion, Reason::UnavailableEnvironment),
            ),
        );

        state.environment.insert(module.analysis.module, status);
        state.completed = state.completed.clone().combine(metadata.completion.clone());

        for finding in &metadata.diagnostics {
            state.prerequisites.insert(finding.identity.clone());
        }

        state
            .findings
            .extend(
                metadata
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| DiagnosticRecord {
                        module: Some(module.analysis.module),
                        diagnostic,
                    }),
            );

        if metadata.completion != Completion::Complete
            || metadata.availability != Availability::Available
        {
            let diagnostic = environments::unavailable(
                &module.parsed.input.source,
                &metadata.diagnostics,
                "requested declaration environment is unavailable".to_owned(),
            );

            state.findings.push(DiagnosticRecord {
                module: Some(module.analysis.module),
                diagnostic,
            });
        }
    } else if context.graph.modules[&module.analysis.module]
        .effective
        .settings
        .linting
        .enabled()
        .any(|(rule, _)| rule.prerequisites.environment || !rule.prerequisites.semantics.is_empty())
    {
        state.environment.insert(
            module.analysis.module,
            Availability::Unavailable(Reason::UnavailableEnvironment),
        );
    }
}

fn fold_lint_environment(recipes: &[ClosureRecipe], state: &mut LintState) {
    for recipe in recipes {
        if recipe
            .request
            .modules
            .iter()
            .any(|module| module.inference.required)
        {
            let status =
                recipe
                    .request
                    .modules
                    .iter()
                    .fold(Availability::Available, |status, module| {
                        combine(
                            status,
                            state
                                .environment
                                .get(&module.analysis.module)
                                .copied()
                                .unwrap_or(Availability::Unavailable(
                                    Reason::UnavailableEnvironment,
                                )),
                        )
                    });

            for module in &recipe.request.modules {
                state.environment.insert(module.analysis.module, status);
            }
        }
    }
}

fn lint_work(
    recipes: &[ClosureRecipe],
    context: &LintContext,
    state: &LintState,
    options: &Options,
) -> (Vec<Work>, Vec<ModuleIdentity>) {
    let mut work = Vec::new();
    let mut identities = Vec::new();

    for recipe in recipes {
        for module in &recipe.request.modules {
            if options.interrupted().is_some() {
                break;
            }

            let identity = module.analysis.module;

            if !state.prepared.contains(&identity) {
                continue;
            }

            let node = &context.graph.modules[&identity];

            let categories = node
                .effective
                .settings
                .linting
                .enabled()
                .filter(|(rule, _)| matches!(rule.producer, RuleProducer::Instar(_)))
                .flat_map(|(rule, _)| rule.prerequisites.semantics.iter().copied())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();

            if !categories.is_empty()
                && module.inference.required
                && state.environment.get(&identity) == Some(&Availability::Available)
            {
                work.push(Work::Facts {
                    closure: recipe.request.clone(),
                    module: identity,
                    categories,
                });

                identities.push(identity);
            }

            let enabled = node
                .effective
                .settings
                .linting
                .upstream()
                .cloned()
                .collect::<Vec<_>>();

            if !enabled.is_empty() {
                work.push(Work::Warnings {
                    closure: recipe.request.clone(),
                    module: identity,
                    warnings: UpstreamWarningSettings {
                        enabled,
                        language_mode: LanguageMode::NoCheck,
                    },
                });

                identities.push(identity);
            }
        }
    }

    (work, identities)
}

fn record_native_warnings(
    mut result: NativeLintResult,
    identity: ModuleIdentity,
    context: &LintContext,
    state: &mut LintState,
) -> io::Result<()> {
    let node = &context.graph.modules[&identity];

    if result.source != node.source.version {
        return Err(io::Error::other(
            "native lint source differs from captured module",
        ));
    }

    result.prerequisite_diagnostics = result
        .prerequisite_diagnostics
        .into_iter()
        .filter(|diagnostic| !checking::duplicate_resolution(diagnostic, &context.graph, identity))
        .map(|diagnostic| checking::prerequisite_resolution(&diagnostic, &context.graph, identity))
        .collect();

    state.completed = state.completed.clone().combine(result.completion.clone());

    accumulate(
        &mut state.producer_completion,
        identity,
        result.completion.clone(),
    );

    state.prerequisites.extend(
        result
            .prerequisite_diagnostics
            .iter()
            .map(|finding| finding.identity.clone()),
    );

    state.findings.extend(
        result
            .prerequisite_diagnostics
            .iter()
            .cloned()
            .map(|diagnostic| DiagnosticRecord {
                module: Some(identity),
                diagnostic,
            }),
    );

    for diagnostic in &result.suppression_findings {
        diagnostic.primary.validate(&node.source)?;

        if diagnostic.category != DiagnosticCategory::Lint
            || diagnostic.identity.producer() != "luau"
            || !node
                .effective
                .settings
                .linting
                .upstream()
                .any(|warning| warning.name.as_str() == diagnostic.code)
        {
            return Err(io::Error::other(
                "native suppression evidence was not selected for its module",
            ));
        }
    }

    state
        .suppression_findings
        .extend(result.suppression_findings);

    record_native_findings(result.findings, identity, context, state)?;
    record_native_availability(result.warnings, identity, context, state)?;

    Ok(())
}

fn record_native_findings(
    findings: Vec<UpstreamFinding>,
    identity: ModuleIdentity,
    context: &LintContext,
    state: &mut LintState,
) -> io::Result<()> {
    let node = &context.graph.modules[&identity];

    for finding in findings {
        let rule = lint::rules().iter().find(|rule| matches!(&rule.producer, RuleProducer::Upstream(warning) if warning == &finding.warning)).ok_or_else(|| io::Error::other("native warning is outside the selected catalog"))?;

        let configuration = node
            .effective
            .settings
            .linting
            .rules
            .get(&rule.identity)
            .filter(|configuration| configuration.enabled)
            .ok_or_else(|| io::Error::other("native warning was not selected for its module"))?;

        if rule.prerequisites.environment
            && matches!(
                state.environment.get(&identity),
                None | Some(Availability::Unavailable(_))
            )
        {
            continue;
        }

        let mut diagnostic = finding.diagnostic;
        diagnostic.primary.validate(&node.source)?;
        diagnostic.code = rule.identity.to_string();
        diagnostic.category = DiagnosticCategory::Lint;
        diagnostic.severity = configuration.severity;

        state.findings.push(DiagnosticRecord {
            module: Some(identity),
            diagnostic,
        });
    }

    Ok(())
}

fn record_native_availability(
    warnings: Vec<WarningAvailability>,
    identity: ModuleIdentity,
    context: &LintContext,
    state: &mut LintState,
) -> io::Result<()> {
    let node = &context.graph.modules[&identity];

    for warning in warnings {
        let rule = lint::rules().iter().find(|rule| matches!(&rule.producer, RuleProducer::Upstream(selected_warning) if selected_warning == &warning.warning)).ok_or_else(|| io::Error::other("native availability is outside the selected catalog"))?;

        if !node
            .effective
            .settings
            .linting
            .rules
            .get(&rule.identity)
            .is_some_and(|configuration| configuration.enabled)
        {
            return Err(io::Error::other(
                "native availability was not selected for its module",
            ));
        }

        state.warnings.insert((identity, rule.identity));

        let status = if rule.prerequisites.environment {
            combine(
                state
                    .environment
                    .get(&identity)
                    .copied()
                    .unwrap_or(Availability::Unavailable(Reason::UnavailableEnvironment)),
                warning.status,
            )
        } else {
            warning.status
        };

        state.availability.push(RuleAvailability {
            rule: rule.identity,
            status,
        });

        if let Availability::Partial(reason) | Availability::Unavailable(reason) = status {
            state.completed = state
                .completed
                .clone()
                .combine(Completion::Incomplete(vec![reason]));

            accumulate(
                &mut state.producer_completion,
                identity,
                Completion::Incomplete(vec![reason]),
            );

            let finding = diagnostic(
                node.source.version,
                "lint.prerequisite",
                format!("{} has incomplete coverage: {reason:?}", rule.identity),
                DiagnosticCategory::Analysis,
                Severity::Error,
                Span { start: 0, end: 0 },
            );

            state.findings.push(DiagnosticRecord {
                module: Some(identity),
                diagnostic: finding,
            });
        }
    }

    Ok(())
}

fn lint_modules(context: &LintContext, state: &mut LintState, options: &Options) {
    for node in context.graph.modules.values() {
        let mut settings = node.effective.settings.linting.clone();

        settings.environment = combine(
            state
                .environment
                .get(&node.identity)
                .copied()
                .unwrap_or(Availability::Unavailable(Reason::UnavailableEnvironment)),
            settings.environment,
        );

        if context.requested_native
            && settings.enabled().any(|(rule, _)| {
                rule.prerequisites.environment || !rule.prerequisites.semantics.is_empty()
            })
            && !state.prepared.contains(&node.identity)
        {
            settings.environment = Availability::Unavailable(Reason::UnavailableEnvironment);
        }

        settings
            .globals
            .extend(node.environment.input.globals.iter().cloned());

        settings.globals.sort();
        settings.globals.dedup();
        let syntax = parse(&node.source, &syntax_settings(&node.effective), options);
        let bindings = bindings(&syntax.tree, options);
        let directives = directives(&syntax.tree, options);

        if options.interrupted().is_none() {
            state
                .suppressions
                .insert(node.source.version, directives.directives.clone());
        }

        let imports = dependencies(&node.imports);

        let result = lint::run(
            &RuleInput {
                source: &node.source,
                syntax: &syntax.tree,
                bindings: &bindings,
                facts: state.facts.get(&node.identity),
                imports: &imports,
                settings: &settings,
                directives: &directives,
            },
            options,
        );

        let mut source_completion = state
            .producer_completion
            .get(&node.identity)
            .cloned()
            .unwrap_or(Completion::Complete)
            .combine(result.completion.clone());

        if settings.enabled().all(|(rule, _)| {
            matches!(rule.producer, RuleProducer::Instar(_))
                && !rule.prerequisites.environment
                && rule.prerequisites.semantics.is_empty()
        }) && let Some(completion) = state.syntax_completion.get(&node.identity)
        {
            source_completion = completion.clone();
        }

        state.findings.extend(
            result
                .findings
                .iter()
                .filter(|finding| {
                    context.graph.roots.contains(&node.identity)
                        || finding.category != DiagnosticCategory::Lint
                })
                .cloned()
                .map(|diagnostic| DiagnosticRecord {
                    module: Some(node.identity),
                    diagnostic,
                }),
        );

        state.completed = state.completed.clone().combine(result.completion);
        state.fixes.extend(result.fixes);

        let evaluated = result
            .availability
            .iter()
            .map(|entry| entry.rule)
            .collect::<BTreeSet<_>>();

        state.availability.extend(result.availability);

        record_rule_coverage(
            &settings,
            node,
            &evaluated,
            &mut source_completion,
            state,
            options,
        );

        if let Some(outcome) = state
            .outcomes
            .iter_mut()
            .find(|outcome| outcome.source == node.source.version)
        {
            outcome.completion = outcome.completion.clone().combine(source_completion);
        } else {
            state.outcomes.push(SourceOutcome {
                source: node.source.version,
                completion: source_completion,
            });
        }
    }
}

fn record_rule_coverage(
    settings: &LintSettings,
    node: &ModuleNode,
    evaluated: &BTreeSet<RuleIdentity>,
    source_completion: &mut Completion,
    state: &mut LintState,
    options: &Options,
) {
    for (rule, _) in settings.enabled() {
        let available = match &rule.producer {
            RuleProducer::Upstream(_) => state.warnings.contains(&(node.identity, rule.identity)),

            RuleProducer::Instar(_) => {
                evaluated.contains(&rule.identity)
                    || state.syntax_completion.contains_key(&node.identity)
                        && !rule.prerequisites.environment
                        && rule.prerequisites.semantics.is_empty()
            }
        };

        if !available {
            let reason = if rule.prerequisites.environment
                && settings.environment != Availability::Available
            {
                Reason::UnavailableEnvironment
            } else {
                Reason::UnavailableSemantics
            };

            state.availability.push(RuleAvailability {
                rule: rule.identity,
                status: Availability::Unavailable(reason),
            });

            *source_completion = source_completion
                .clone()
                .combine(Completion::Incomplete(vec![reason]));

            state.completed = state
                .completed
                .clone()
                .combine(Completion::Incomplete(vec![reason]));

            if options.interrupted().is_none() {
                let finding = diagnostic(
                    node.source.version,
                    "lint.prerequisite",
                    format!("{} has unavailable requested coverage", rule.identity),
                    DiagnosticCategory::Analysis,
                    Severity::Error,
                    Span { start: 0, end: 0 },
                );

                state.findings.push(DiagnosticRecord {
                    module: Some(node.identity),
                    diagnostic: finding,
                });
            }
        }
    }
}

fn stream_lint_outcomes(context: &LintContext, state: &mut LintState, options: &Options) {
    for decision in context
        .selected
        .decisions
        .iter()
        .filter(|decision| !decision.selected)
    {
        if let Some(identity) = decision.source
            && matches!(
                context.capture.snapshot.origin(identity),
                Some(SourceOrigin::Stdin { .. })
            )
            && let Some(source) = context.capture.snapshot.source(identity)
        {
            state.outcomes.push(SourceOutcome {
                source: source.version,
                completion: Completion::Complete,
            });
        }
    }

    for outcome in &state.outcomes {
        options.report(Event {
            invocation: 0,
            operation: 0,
            payload: EventPayload::SourceOutcome(outcome.clone()),
        });
    }

    if let Some(interrupted) = options.interrupted() {
        state.completed = state.completed.clone().combine(interrupted);
    }
}

fn filter_lint_findings(context: &LintContext, state: &mut LintState) {
    state.findings.retain(|finding| {
        retained(&finding.diagnostic, &context.ordinary, &state.prerequisites)
            && !(finding.diagnostic.category == DiagnosticCategory::Syntax
                && finding.diagnostic.code == "syntax.parse"
                && finding.diagnostic.identity.producer() == "syntax"
                && state
                    .native_syntax
                    .contains(&finding.diagnostic.primary.source))
    });

    let prerequisites = checking::dependency_findings(&context.graph, &state.findings);
    state.findings.extend(prerequisites);

    let directive_diagnostics = suppression_diagnostics(
        &state
            .findings
            .iter()
            .map(|finding| finding.diagnostic.clone())
            .collect::<Vec<_>>(),
        &state.suppression_findings,
        &state.suppressions,
        &context.capture.snapshot,
        state.completed == Completion::Complete,
    );

    for diagnostic in directive_diagnostics {
        let modules = context
            .graph
            .modules
            .values()
            .filter(|node| node.source.version == diagnostic.primary.source)
            .map(|node| node.identity)
            .collect::<Vec<_>>();

        if modules.is_empty() {
            state.findings.push(DiagnosticRecord {
                module: None,
                diagnostic,
            });
        } else {
            state
                .findings
                .extend(modules.into_iter().map(|module| DiagnosticRecord {
                    module: Some(module),
                    diagnostic: diagnostic.clone(),
                }));
        }
    }
}

fn lint_report(
    request: &LintRequest,
    context: &mut LintContext,
    state: &mut LintState,
    options: &Options,
) -> LintReport {
    let findings = prepared(
        mem::take(&mut state.findings),
        &state.suppressions,
        &context.capture.snapshot,
    );

    let module_findings = associated(&findings, &context.graph, &request.overrides.diagnostics);

    let (findings, failed) = reporting(
        findings,
        &context.graph,
        &request.overrides.diagnostics,
        &mut state.emitted,
        options,
    );

    let mut report = LintReport {
        sources: context
            .graph
            .modules
            .values()
            .map(|node| node.source.version)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect(),
        modules: context.graph.modules.keys().copied().collect(),
        module_findings,
        imports: mem::take(&mut context.graph.imports),
        outcomes: mem::take(&mut state.outcomes),
        snapshots: vec![context.capture.snapshot.clone()],
        snapshot: context.capture.snapshot.clone(),
        inputs: context.capture.guards(),
        findings: findings.clone(),
        availability: mem::take(&mut state.availability),
        fixes: mem::take(&mut state.fixes),
        proposals: Vec::new(),
        previews: Vec::new(),
        writes: Vec::new(),
        final_findings: findings,
        selection: mem::take(&mut context.selected.decisions),
        settings: context
            .graph
            .modules
            .values()
            .map(|node| node.effective.clone())
            .collect(),
        dependencies: mem::take(&mut context.graph.edges),
        counts: Counts {
            selected: context.graph.roots.len(),
            dependencies: context
                .graph
                .modules
                .len()
                .saturating_sub(context.graph.roots.len()),
            excluded: context
                .selected
                .candidates
                .saturating_sub(context.selected.targets.len()),
            ..Counts::default()
        },
        completion: state.completed.clone(),
        failed,
    };

    report.counts.findings = report.final_findings.len();

    report
}

fn record_fix_failure(
    request: &LintRequest,
    context: &LintContext,
    state: &mut LintState,
    report: &mut LintReport,
    error: &io::Error,
    options: &Options,
) {
    let completion = options
        .interrupted()
        .unwrap_or(Completion::Incomplete(vec![
            if error.kind() == io::ErrorKind::Unsupported {
                Reason::UnavailableSemantics
            } else {
                Reason::FailedPrerequisite
            },
        ]));

    report.completion = report.completion.clone().combine(completion.clone());

    let affected = report
        .fixes
        .iter()
        .filter(|fix| {
            request
                .fix_rules
                .as_ref()
                .is_none_or(|rules| rules.contains(&fix.rule))
        })
        .flat_map(|fix| {
            fix.proposal
                .edits
                .iter()
                .map(|edit| edit.location.source.identity)
        })
        .collect::<BTreeSet<_>>();

    for outcome in &mut report.outcomes {
        if affected.contains(&outcome.source.identity) {
            outcome.completion = outcome.completion.clone().combine(completion.clone());

            options.report(Event {
                invocation: 0,
                operation: 0,
                payload: EventPayload::SourceOutcome(outcome.clone()),
            });
        }
    }

    if options.interrupted().is_none()
        && let Some(source) = report
            .sources
            .iter()
            .find(|source| affected.contains(&source.identity))
    {
        let finding = diagnostic(
            *source,
            "lint.fix",
            error.to_string(),
            DiagnosticCategory::Edit,
            Severity::Error,
            Span { start: 0, end: 0 },
        );

        let (additional, failed) = reporting(
            prepared(
                vec![DiagnosticRecord {
                    module: None,
                    diagnostic: finding,
                }],
                &state.suppressions,
                &report.snapshot,
            ),
            &context.graph,
            &request.overrides.diagnostics,
            &mut state.emitted,
            options,
        );

        report.failed |= failed;

        for finding in &additional {
            report.module_findings.extend(
                report
                    .modules
                    .iter()
                    .filter(|module| module.source == finding.primary.source.identity)
                    .map(|module| ModuleDiagnostic {
                        module: *module,
                        diagnostic: finding.clone(),
                    }),
            );
        }

        report.final_findings.extend(additional);
    }
}

fn finish_lint_failures(context: LintContext, report: LintReport) -> io::Result<LintReport> {
    let mut failures = context.failures.into_iter();

    if let Some(mut failure) = failures.next() {
        for additional in failures {
            if additional.message != failure.message {
                failure.message.push('\n');
                failure.message.push_str(&additional.message);
            }

            failure.completion = failure.completion.combine(additional.completion);

            for diagnostic in additional.diagnostics {
                if !failure.diagnostics.contains(&diagnostic) {
                    failure.diagnostics.push(diagnostic);
                }
            }

            for outcome in additional.completed_sources {
                if !failure.completed_sources.contains(&outcome) {
                    failure.completed_sources.push(outcome);
                }
            }

            for outcome in additional.completed_edits {
                if !failure.completed_edits.contains(&outcome) {
                    failure.completed_edits.push(outcome);
                }
            }
        }

        failure.completion = failure.completion.combine(report.completion);

        for diagnostic in report.final_findings {
            if !failure.diagnostics.contains(&diagnostic) {
                failure.diagnostics.push(diagnostic);
            }
        }

        for outcome in report.outcomes {
            if !failure.completed_sources.contains(&outcome) {
                failure.completed_sources.push(outcome);
            }
        }

        return Err(failure.into());
    }

    Ok(report)
}

fn collect_lint_fixes(
    request: &LintRequest,
    targets: &BTreeSet<SourceIdentity>,
    report: &LintReport,
    options: &Options,
) -> io::Result<Vec<RuleFix>> {
    options.check()?;

    let rules = request
        .fix_rules
        .clone()
        .unwrap_or_else(|| report.fixes.iter().map(|fix| fix.rule).collect());

    let candidates = report
        .fixes
        .iter()
        .filter(|fix| {
            fix.proposal.edits.iter().all(|edit| {
                targets.contains(&edit.location.source.identity)
                    && (!matches!(request.fixes, FixMode::Stdin)
                        || matches!(
                            report.snapshot.origin(edit.location.source.identity),
                            Some(SourceOrigin::Stdin { .. })
                        ))
            })
        })
        .cloned()
        .collect::<Vec<_>>();

    let fixes = lint::collect_fixes(
        &candidates,
        &rules.into_iter().collect::<Vec<_>>(),
        &report.snapshot.sources().cloned().collect::<Vec<_>>(),
        options,
    )?;

    Ok(fixes)
}

fn preview_lint_fixes(
    validated: &ValidatedEdit,
    changed: &BTreeSet<SourceIdentity>,
    report: &mut LintReport,
    options: &Options,
) -> io::Result<()> {
    for identity in changed {
        options.check()?;

        let original = report
            .snapshot
            .source(*identity)
            .ok_or_else(|| io::Error::other("fix original source is absent"))?;

        let proposed = validated
            .proposed()
            .source(*identity)
            .ok_or_else(|| io::Error::other("fix proposed source is absent"))?;

        report.previews.push(preview(
            &original.text,
            &proposed.text,
            &report
                .snapshot
                .origin(*identity)
                .and_then(SourceOrigin::path)
                .map_or_else(|| "stdin".to_owned(), |path| path.display().to_string()),
            options,
        )?);
    }

    Ok(())
}

fn accept_stdin_fixes(
    validated: &ValidatedEdit,
    changed: &BTreeSet<SourceIdentity>,
    report: &LintReport,
    application: &mut FixApplication,
    options: &Options,
) -> io::Result<()> {
    let mut replacements = Vec::new();

    for identity in changed {
        if matches!(
            report.snapshot.origin(*identity),
            Some(SourceOrigin::Stdin { .. })
        ) {
            options.check()?;

            let original = application
                .snapshot
                .source(*identity)
                .ok_or_else(|| io::Error::other("stdin source is absent from accepted snapshot"))?;

            let proposed = validated
                .proposed()
                .source(*identity)
                .ok_or_else(|| io::Error::other("proposed stdin is absent"))?;

            replacements.push(ProjectChange::Source(SourceChange::Replace {
                expected: original.version,
                text: Arc::clone(&proposed.text),
            }));

            application.accepted.insert(*identity);
        }
    }

    if !replacements.is_empty() {
        application.snapshot = application.snapshot.transition(ChangeBatch {
            changes: replacements,
        })?;
    }

    Ok(())
}

fn dependencies(imports: &[ResolvedImport]) -> Vec<ResolvedDependency> {
    imports
        .iter()
        .map(|import| ResolvedDependency {
            call: import.site.call,
            argument: import.site.argument,
            constant: !matches!(import.site.request, ImportRequest::Nonconstant)
                && !matches!(import.outcome, ImportOutcome::Nonconstant),
            requested: Knowledge::Known(
                if let ImportRequest::Static(path) = &import.site.request {
                    Some(path.clone())
                } else {
                    None
                },
            ),
            target: Knowledge::Known(if let ImportOutcome::Module(module) = import.outcome {
                Some(module)
            } else {
                None
            }),
            availability: if matches!(import.outcome, ImportOutcome::Unresolved) {
                Availability::Unavailable(Reason::FailedPrerequisite)
            } else {
                Availability::Available
            },
        })
        .collect()
}

fn coverage(completion: &Completion, reason: Reason) -> Availability {
    match completion {
        Completion::Complete => Availability::Available,

        Completion::Incomplete(reasons) => {
            Availability::Unavailable(reasons.first().copied().unwrap_or(reason))
        }

        Completion::Cancelled | Completion::TimedOut => Availability::Unavailable(reason),
    }
}

fn combine(left: Availability, right: Availability) -> Availability {
    match (left, right) {
        (Availability::Unavailable(reason), _) | (_, Availability::Unavailable(reason)) => {
            Availability::Unavailable(reason)
        }

        (Availability::Partial(reason), _) | (_, Availability::Partial(reason)) => {
            Availability::Partial(reason)
        }

        _ => Availability::Available,
    }
}

fn accumulate(
    completions: &mut BTreeMap<ModuleIdentity, Completion>,
    module: ModuleIdentity,
    next: Completion,
) {
    let completion = completions.entry(module).or_insert(Completion::Complete);
    *completion = completion.clone().combine(next);
}

fn progress(options: &Options, completed: usize, total: usize) {
    options.report(Event {
        invocation: 0,
        operation: 0,
        payload: EventPayload::Progress(Progress {
            source: None,
            phase: "Syntax linting".to_owned(),
            completed,
            total: Some(total),
        }),
    });
}

fn family(finding: &Diagnostic) -> String {
    if finding.category == DiagnosticCategory::Lint
        && matches!(
            finding.code.as_str(),
            "unused_variable" | "LocalUnused" | "FunctionUnused" | "ImportUnused"
        )
    {
        "unused".to_owned()
    } else {
        format!("{}:{}", finding.code, finding.message)
    }
}

fn precedence(settings: &EffectiveSettings, code: &str) -> usize {
    let field = format!("linting.rules.{code}.");

    settings
        .origins
        .iter()
        .filter(|origin| origin.field.starts_with(&field))
        .map(|origin| origin.precedence)
        .max()
        .unwrap_or(0)
}

fn associated(
    original: &[DiagnosticRecord],
    graph: &DependencyGraph,
    policy: &DiagnosticPolicy,
) -> Vec<ModuleDiagnostic> {
    let mut grouped =
        BTreeMap::<(ModuleIdentity, Location, DiagnosticCategory, String), ModuleDiagnostic>::new();

    for record in original {
        let Some(module) = record.module else {
            continue;
        };

        let association = ModuleDiagnostic {
            module,
            diagnostic: record.diagnostic.clone(),
        };

        let finding = &association.diagnostic;

        let key = (
            association.module,
            finding.primary,
            finding.category,
            family(finding),
        );

        if let Some(previous) = grouped.get_mut(&key) {
            let settings = &graph.modules[&association.module].effective;
            let previous_precedence = precedence(settings, &previous.diagnostic.code);
            let next_precedence = precedence(settings, &finding.code);

            merge(
                &mut previous.diagnostic,
                association.diagnostic,
                previous_precedence,
                next_precedence,
            );
        } else {
            grouped.insert(key, association);
        }
    }

    grouped
        .into_values()
        .filter(|association| displayed(&association.diagnostic, policy))
        .collect()
}

fn retained(
    finding: &Diagnostic,
    ordinary: &BTreeSet<SourceIdentity>,
    prerequisites: &BTreeSet<FindingIdentity>,
) -> bool {
    finding.category != DiagnosticCategory::Type
        || !ordinary.contains(&finding.primary.source.identity)
        || prerequisites.contains(&finding.identity)
}

fn displayed(finding: &Diagnostic, policy: &DiagnosticPolicy) -> bool {
    policy
        .codes
        .as_ref()
        .is_none_or(|codes| codes.contains(&finding.code))
        && policy
            .categories
            .as_ref()
            .is_none_or(|categories| categories.contains(&finding.category))
        && policy
            .severities
            .as_ref()
            .is_none_or(|severities| severities.contains(&finding.severity))
}

fn suppressed(
    finding: &Diagnostic,
    suppressions: &BTreeMap<SourceVersion, Vec<Directive>>,
    snapshot: &ProjectSnapshot,
) -> bool {
    finding.category == DiagnosticCategory::Lint
        && suppressions
            .get(&finding.primary.source)
            .is_some_and(|directives| {
                directives.iter().any(|directive| {
                    let DirectiveKind::Lint(names) = &directive.kind else {
                        return false;
                    };

                    let native_only = snapshot
                        .source(finding.primary.source.identity)
                        .and_then(|source| {
                            source
                                .text
                                .get(directive.location.start..directive.location.end)
                        })
                        .is_some_and(|text| {
                            text.trim_start_matches('-')
                                .trim_start()
                                .starts_with("!nolint")
                        });

                    (!native_only
                        || lint::rules().iter().any(|rule| {
                            rule.identity.name() == finding.code
                                && matches!(rule.producer, RuleProducer::Upstream(_))
                        }))
                        && (names.is_empty() || names.contains(&finding.code))
                        && directive.scope.start <= finding.primary.span.start
                        && finding.primary.span.end <= directive.scope.end
                })
            })
}

fn prepared(
    findings: Vec<DiagnosticRecord>,
    suppressions: &BTreeMap<SourceVersion, Vec<Directive>>,
    snapshot: &ProjectSnapshot,
) -> Vec<DiagnosticRecord> {
    findings
        .into_iter()
        .filter_map(|mut finding| {
            canonicalize(&mut finding.diagnostic);

            (!suppressed(&finding.diagnostic, suppressions, snapshot)).then_some(finding)
        })
        .collect()
}

fn reporting(
    findings: Vec<DiagnosticRecord>,
    graph: &DependencyGraph,
    policy: &DiagnosticPolicy,
    emitted: &mut BTreeSet<FindingIdentity>,
    options: &Options,
) -> (Vec<Diagnostic>, bool) {
    let mut grouped = BTreeMap::<(Location, DiagnosticCategory, String), Diagnostic>::new();

    let finding_precedence = |finding: &Diagnostic| {
        graph
            .modules
            .values()
            .filter(|node| node.source.version == finding.primary.source)
            .map(|node| precedence(&node.effective, &finding.code))
            .max()
            .unwrap_or(0)
    };

    for record in findings {
        let finding = record.diagnostic;

        let key = (finding.primary, finding.category, family(&finding));

        if let Some(previous) = grouped.get_mut(&key) {
            let previous_precedence = finding_precedence(previous);
            let next_precedence = finding_precedence(&finding);

            merge(previous, finding, previous_precedence, next_precedence);
        } else {
            grouped.insert(key, finding);
        }
    }

    let mut failed = false;

    let findings = grouped
        .into_values()
        .filter(|finding| {
            failed |= finding.severity <= policy.threshold
                || policy.warnings_as_errors && finding.severity == Severity::Warning;

            displayed(finding, policy)
        })
        .flat_map(|finding| {
            let placements = graph
                .roots
                .iter()
                .filter(|module| module.source == finding.primary.source.identity)
                .count();

            iter::repeat_n(finding, placements.max(1))
        })
        .collect::<Vec<_>>();

    for finding in &findings {
        if emitted.insert(finding.identity.clone()) {
            options.report(Event {
                invocation: 0,
                operation: 0,
                payload: EventPayload::Diagnostic(finding.clone()),
            });
        }
    }

    (findings, failed)
}

fn merge(
    previous: &mut Diagnostic,
    mut next: Diagnostic,
    previous_precedence: usize,
    next_precedence: usize,
) {
    let severity = if next_precedence > previous_precedence {
        next.severity
    } else if previous_precedence > next_precedence {
        previous.severity
    } else {
        previous.severity.min(next.severity)
    };

    let mut related = mem::take(&mut previous.related);
    related.append(&mut next.related);
    let mut tags = mem::take(&mut previous.tags);
    tags.append(&mut next.tags);

    if next_precedence > previous_precedence
        || next_precedence == previous_precedence
            && (next.code == "unused_variable", &next.code, &next.identity)
                < (
                    previous.code == "unused_variable",
                    &previous.code,
                    &previous.identity,
                )
    {
        *previous = next;
    }

    previous.severity = severity;
    previous.related = related;
    previous.tags = tags;
    canonicalize(previous);
}

fn canonicalize(finding: &mut Diagnostic) {
    finding.related.sort_by(|left, right| {
        left.location
            .cmp(&right.location)
            .then_with(|| left.message.cmp(&right.message))
    });

    finding.related.dedup();

    finding.tags.sort_by_key(|tag| match tag {
        DiagnosticTag::Unnecessary => 0,
        DiagnosticTag::Deprecated => 1,
    });

    finding.tags.dedup();
}

fn suppression_diagnostics(
    findings: &[Diagnostic],
    suppression_findings: &[Diagnostic],
    suppressions: &BTreeMap<SourceVersion, Vec<Directive>>,
    snapshot: &ProjectSnapshot,
    complete: bool,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();

    for (source, directives) in suppressions {
        for directive in directives {
            let DirectiveKind::Lint(names) = &directive.kind else {
                continue;
            };

            let native_only = snapshot
                .source(source.identity)
                .and_then(|source| {
                    source
                        .text
                        .get(directive.location.start..directive.location.end)
                })
                .is_some_and(|text| {
                    text.trim_start_matches('-')
                        .trim_start()
                        .starts_with("!nolint")
                });

            for name in names {
                if !lint::rules().iter().any(|rule| {
                    rule.identity.name() == name
                        && (!native_only || matches!(rule.producer, RuleProducer::Upstream(_)))
                }) {
                    diagnostics.push(diagnostic(
                        *source,
                        "lint.directive",
                        format!("unknown lint rule {name}"),
                        DiagnosticCategory::Analysis,
                        Severity::Error,
                        directive.location,
                    ));
                } else if complete
                    && !findings.iter().chain(suppression_findings).any(|finding| {
                        finding.primary.source == *source
                            && finding.code == *name
                            && suppressed(
                                finding,
                                &BTreeMap::from([(*source, vec![directive.clone()])]),
                                snapshot,
                            )
                    })
                {
                    diagnostics.push(diagnostic(
                        *source,
                        "lint.directive",
                        format!("ineffective suppression for {name}"),
                        DiagnosticCategory::Analysis,
                        Severity::Warning,
                        directive.location,
                    ));
                }
            }

            if names.is_empty()
                && complete
                && !findings.iter().chain(suppression_findings).any(|finding| {
                    finding.primary.source == *source
                        && suppressed(
                            finding,
                            &BTreeMap::from([(*source, vec![directive.clone()])]),
                            snapshot,
                        )
                })
            {
                diagnostics.push(diagnostic(
                    *source,
                    "lint.directive",
                    "ineffective lint suppression".to_owned(),
                    DiagnosticCategory::Analysis,
                    Severity::Warning,
                    directive.location,
                ));
            }
        }
    }

    diagnostics
}

fn diagnostic(
    source: SourceVersion,
    code: &str,
    message: String,
    category: DiagnosticCategory,
    severity: Severity,
    span: Span,
) -> Diagnostic {
    let primary = Location { source, span };

    Diagnostic {
        identity: FindingIdentity::new(
            "project",
            vec![source],
            format!("{code}:{message}"),
            primary,
        ),
        code: code.to_owned(),
        category,
        severity,
        message,
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use instar_shared::{
        DiagnosticCategory, DiagnosticTag, Location, RelatedLocation, Severity, SourceIdentity,
        SourceVersion, Span,
    };

    use super::{diagnostic, merge};

    #[test]
    fn diagnostic_merge_preserves_precedence_and_canonical_metadata_in_both_orders() {
        let source = SourceVersion {
            identity: SourceIdentity::new(1),
            revision: 1,
        };

        let related = [2, 3].map(|revision| RelatedLocation {
            location: Location {
                source: SourceVersion { revision, ..source },
                span: Span { start: 0, end: 1 },
            },
            message: format!("revision {revision}"),
        });

        let mut instar = diagnostic(
            source,
            "unused_variable",
            "unused local".to_owned(),
            DiagnosticCategory::Lint,
            Severity::Warning,
            Span { start: 0, end: 1 },
        );

        instar.related = vec![related[1].clone(), related[0].clone()];
        instar.tags = vec![DiagnosticTag::Deprecated, DiagnosticTag::Unnecessary];

        let mut native = diagnostic(
            source,
            "LocalUnused",
            "unused local".to_owned(),
            DiagnosticCategory::Lint,
            Severity::Error,
            Span { start: 0, end: 1 },
        );

        native.related = vec![related[0].clone(), related[1].clone()];
        native.tags = vec![DiagnosticTag::Unnecessary, DiagnosticTag::Deprecated];

        for (instar_precedence, native_precedence, code, severity) in [
            (0, 0, "LocalUnused", Severity::Error),
            (1, 0, "unused_variable", Severity::Warning),
            (0, 1, "LocalUnused", Severity::Error),
        ] {
            let mut forward = instar.clone();

            merge(
                &mut forward,
                native.clone(),
                instar_precedence,
                native_precedence,
            );

            let mut reverse = native.clone();

            merge(
                &mut reverse,
                instar.clone(),
                native_precedence,
                instar_precedence,
            );

            assert_eq!(forward, reverse);
            assert_eq!(forward.code, code);
            assert_eq!(forward.severity, severity);
            assert_eq!(forward.related, related);

            assert_eq!(
                forward.tags,
                vec![DiagnosticTag::Unnecessary, DiagnosticTag::Deprecated],
            );
        }
    }
}
