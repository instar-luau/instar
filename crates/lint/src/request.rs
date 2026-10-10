use std::io;

use instar_luau::{FactsResult, ModuleIdentity};

use instar_shared::{
    Availability, Completion, Diagnostic, DiagnosticCategory, Event, EventPayload, FindingIdentity,
    Knowledge, Location, Options, Progress, Reason, Severity, SourceSnapshot, Span, error::invalid,
};

use instar_syntax::{
    BindingResult, DirectiveResult, Tree, literal,
    token::Span as SyntaxSpan,
    tree::{NodeIndex, NodeKind, Tree as SyntaxTree},
};

use crate::{
    LintSettings, Prerequisites, RuleAvailability, RuleIdentity, RuleProducer, RuleResult,
    RuleSettings, fixes::propose,
};

/// A detached import result supplied by project resolution.
#[derive(Clone, Debug)]
pub struct ResolvedDependency {
    /// Full require-call location.
    pub call: Location,

    /// Argument expression location.
    pub argument: Location,

    /// Whether the supplied typed request is a constant path or instance.
    pub constant: bool,

    /// Decoded string path, established absence, or unavailable extraction.
    pub requested: Knowledge<Option<String>>,

    /// Resolved module identity or explicit unresolved knowledge.
    pub target: Knowledge<Option<ModuleIdentity>>,

    /// Resolution coverage independent of nonconstant request classification.
    pub availability: Availability,
}

/// Exact captured evidence supplied by the caller without hidden analysis.
pub struct RuleInput<'source, 'tree> {
    /// Immutable source bytes and their exact revision.
    pub source: &'source SourceSnapshot,

    /// Request-local lossless syntax for the same snapshot.
    pub syntax: &'tree Tree<'source>,

    /// Captured lexical declarations, references, and writes.
    pub bindings: &'tree BindingResult,

    /// Optional established native semantics, never an implicit inference request.
    pub facts: Option<&'tree FactsResult>,

    /// Project-resolved detached import records.
    pub imports: &'tree [ResolvedDependency],

    /// Validated effective rule policy.
    pub settings: &'tree LintSettings,

    /// Captured source directives and their prerequisite diagnostics.
    pub directives: &'tree DirectiveResult,
}

pub(crate) struct Context<'tree, 'source> {
    pub(crate) tree: &'tree SyntaxTree<'source>,
    pub(crate) source: &'tree SourceSnapshot,
    pub(crate) bindings: &'tree BindingResult,
    pub(crate) facts: Option<&'tree FactsResult>,
    pub(crate) imports: &'tree [ResolvedDependency],
    pub(crate) policy: &'tree LintSettings,
    pub(crate) parameters: &'tree RuleSettings,
    pub(crate) rule: RuleIdentity,
    pub(crate) result: RuleResult,
}

impl<'tree, 'source> Context<'tree, 'source> {
    pub(crate) fn new(
        input: &'tree RuleInput<'source, '_>,
        parameters: &'tree RuleSettings,
    ) -> Self {
        Self {
            tree: input.syntax.syntax(),
            source: input.source,
            bindings: input.bindings,
            facts: input.facts,
            imports: input.imports,
            policy: input.settings,
            parameters,
            rule: parameters.identity(),
            result: RuleResult::default(),
        }
    }

    pub(crate) fn emit(
        &mut self,
        span: SyntaxSpan,
        rule: RuleIdentity,
        message: impl Into<String>,
    ) {
        if self.rule != rule {
            return;
        }

        let primary = Location {
            source: self.source.version,
            span: Span {
                start: span.start,
                end: span.end,
            },
        };

        if primary.validate(self.source).is_err() {
            self.unavailable(Reason::FailedPrerequisite);

            return;
        }

        let code = rule.name().to_owned();

        self.result.findings.push(Diagnostic {
            identity: FindingIdentity::new("instar", vec![self.source.version], &code, primary),
            code,
            category: DiagnosticCategory::Lint,
            severity: self
                .policy
                .rules
                .get(&rule)
                .map_or(Severity::Warning, |configuration| configuration.severity),
            message: message.into(),
            primary,
            related: Vec::new(),
            tags: Vec::new(),
        });
    }

    pub(crate) fn finding(
        &mut self,
        node: NodeIndex,
        rule: RuleIdentity,
        message: impl Into<String>,
    ) {
        self.emit(self.tree.node(node).span, rule, message);
    }

    pub(crate) fn global(&self, node: NodeIndex, name: &[u8]) -> bool {
        matches!(self.tree.node(node).kind, NodeKind::Name { .. })
            && self.tree.text(node) == name
            && self.bindings.global(self.tree, node)
    }

    pub(crate) fn unavailable(&mut self, reason: Reason) {
        self.result.completion = self
            .result
            .completion
            .clone()
            .combine(Completion::Incomplete(vec![reason]));
    }

    pub(crate) fn walk(
        &mut self,
        options: &Options,
        check: fn(&mut Context<'_, '_>, NodeIndex, &[NodeIndex]),
    ) {
        let mut pending = vec![(self.tree.root, Vec::new())];

        while let Some((node, mut ancestors)) = pending.pop() {
            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);
                break;
            }

            if matches!(
                self.rule,
                RuleIdentity::DivideByZero
                    | RuleIdentity::CompareNan
                    | RuleIdentity::ZeroStepLoop
                    | RuleIdentity::ConstantCondition
                    | RuleIdentity::RobloxIncorrectColor3NewBounds
                    | RuleIdentity::RobloxManualFromscaleOrFromoffset
            ) && matches!(self.tree.node(node).kind, NodeKind::Number { .. })
                && !matches!(
                    literal::number(self.tree, node),
                    Some(literal::NumberValue::Float(_))
                )
            {
                self.unavailable(Reason::UnsupportedFeature);
            }

            check(self, node, &ancestors);
            ancestors.push(node);

            for child in self.tree.children(node).into_iter().rev() {
                pending.push((child, ancestors.clone()));
            }
        }
    }

    pub(crate) fn finish(mut self, options: &Options) -> RuleResult {
        self.result.findings.sort_by_key(|finding| {
            (
                finding.primary.span.start,
                finding.primary.span.end,
                finding.code.clone(),
            )
        });

        self.result
            .findings
            .dedup_by(|left, right| left.identity == right.identity);

        for (completed, finding) in self.result.findings.iter().enumerate() {
            report(options, self.source, completed, self.result.findings.len());

            if let Some(completion) = options.interrupted() {
                self.result.completion = self.result.completion.clone().combine(completion);
                break;
            }

            if let Some(fix) = propose(self.rule, finding, self.tree, self.bindings, options) {
                self.result.fixes.push(fix);
            }

            report(
                options,
                self.source,
                completed + 1,
                self.result.findings.len(),
            );
        }

        if let Some(completion) = options.interrupted() {
            self.result.completion = self.result.completion.clone().combine(completion);
        }

        self.result
    }
}

/// Evaluates enabled Instar producers using only supplied records.
#[must_use]
pub fn run(input: &RuleInput<'_, '_>, options: &Options) -> RuleResult {
    let mut result = RuleResult::default();

    if let Some(completion) = options.interrupted() {
        result.completion = completion;

        return result;
    }

    if let Err(error) = validate(input, options) {
        if let Some(completion) = options.interrupted() {
            result.completion = completion;

            return result;
        }

        result
            .findings
            .push(prerequisite(input, "lint.input", error.to_string()));

        result.completion = Completion::Incomplete(vec![Reason::FailedPrerequisite]);

        for (definition, _) in input.settings.enabled() {
            if matches!(definition.producer, RuleProducer::Instar(_)) {
                result.availability.push(RuleAvailability {
                    rule: definition.identity,
                    status: Availability::Unavailable(Reason::FailedPrerequisite),
                });
            }
        }

        return result;
    }

    collect_diagnostics(input, &mut result);

    for (definition, configuration) in input.settings.enabled() {
        let RuleProducer::Instar(evaluate) = &definition.producer else {
            continue;
        };

        if let Some(completion) = options.interrupted() {
            result.completion = result.completion.combine(completion);
            break;
        }

        let mut status = availability(input, definition.prerequisites, &mut result);

        if !matches!(status, Availability::Unavailable(_))
            || !definition.prerequisites.semantics.is_empty()
                && input.settings.environment == Availability::Available
                && input.syntax.coverage() == &Completion::Complete
                && input.bindings.completion == Completion::Complete
        {
            let evaluated = evaluate(input, &configuration.settings, options);

            if evaluated.completion != Completion::Complete && status == Availability::Available {
                let reason = match &evaluated.completion {
                    Completion::Incomplete(reasons) => reasons
                        .first()
                        .copied()
                        .unwrap_or(Reason::FailedPrerequisite),

                    Completion::Cancelled | Completion::TimedOut => Reason::FailedPrerequisite,
                    Completion::Complete => Reason::UnavailableSemantics,
                };

                status = Availability::Partial(reason);
            }

            result.combine(evaluated);
        }

        if let Availability::Partial(reason) | Availability::Unavailable(reason) = status {
            if !matches!(
                result.completion,
                Completion::Cancelled | Completion::TimedOut
            ) {
                result.findings.push(prerequisite(
                    input,
                    "lint.prerequisite",
                    format!(
                        "{} has incomplete coverage: {reason:?}",
                        definition.identity
                    ),
                ));
            }

            result.completion = result
                .completion
                .combine(Completion::Incomplete(vec![reason]));
        }

        result.availability.push(RuleAvailability {
            rule: definition.identity,
            status,
        });
    }

    if let Some(completion) = options.interrupted() {
        result.completion = result.completion.combine(completion);
    }

    result
}

fn collect_diagnostics(input: &RuleInput<'_, '_>, result: &mut RuleResult) {
    result
        .findings
        .extend(input.directives.diagnostics.iter().cloned());

    result.completion = result
        .completion
        .clone()
        .combine(input.directives.completion.clone());

    for diagnostic in &input.syntax.syntax().diagnostics {
        let primary = Location {
            source: input.source.version,
            span: Span {
                start: diagnostic.span.start,
                end: diagnostic.span.end,
            },
        };

        result.findings.push(Diagnostic {
            identity: FindingIdentity::new(
                "syntax",
                vec![input.source.version],
                "syntax.parse",
                primary,
            ),
            code: "syntax.parse".to_owned(),
            category: DiagnosticCategory::Syntax,
            severity: Severity::Error,
            message: diagnostic.message.to_owned(),
            primary,
            related: Vec::new(),
            tags: Vec::new(),
        });
    }

    if let Some(facts) = input.facts {
        result.findings.extend(
            facts
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.category != DiagnosticCategory::Type)
                .cloned(),
        );
    }
}

fn availability(
    input: &RuleInput<'_, '_>,
    prerequisites: Prerequisites,
    result: &mut RuleResult,
) -> Availability {
    let mut status = Availability::Available;

    if prerequisites.bindings {
        status = combine_availability(status, input.bindings.availability);

        result.completion = result
            .completion
            .clone()
            .combine(input.bindings.completion.clone());
    }

    if prerequisites.syntax && input.syntax.coverage() != &Completion::Complete {
        status = combine_availability(
            status,
            Availability::Unavailable(Reason::UnsupportedFeature),
        );

        result.completion = result
            .completion
            .clone()
            .combine(input.syntax.coverage().clone());
    }

    if prerequisites.syntax && !input.syntax.syntax().diagnostics.is_empty() {
        status = combine_availability(status, Availability::Partial(Reason::UnsupportedSyntax));
    }

    if prerequisites.environment {
        status = combine_availability(status, input.settings.environment);
    }

    if !prerequisites.semantics.is_empty()
        && let Some(facts) = input.facts
    {
        result.completion = result.completion.clone().combine(facts.completion.clone());

        if facts.completion != Completion::Complete {
            status =
                combine_availability(status, Availability::Partial(Reason::FailedPrerequisite));
        }
    }

    for category in prerequisites.semantics {
        let available = input
            .facts
            .and_then(|facts| facts.availability.categories.get(category))
            .copied()
            .unwrap_or(Availability::Unavailable(Reason::UnavailableSemantics));

        status = combine_availability(status, available);
    }

    status
}

fn combine_availability(current: Availability, next: Availability) -> Availability {
    match (current, next) {
        (Availability::Unavailable(reason), _) | (_, Availability::Unavailable(reason)) => {
            Availability::Unavailable(reason)
        }

        (Availability::Partial(reason), _) | (_, Availability::Partial(reason)) => {
            Availability::Partial(reason)
        }

        _ => Availability::Available,
    }
}

fn prerequisite(input: &RuleInput<'_, '_>, code: &str, message: String) -> Diagnostic {
    let primary = Location {
        source: input.source.version,
        span: Span { start: 0, end: 0 },
    };

    Diagnostic {
        identity: FindingIdentity::new(
            "lint",
            vec![input.source.version],
            format!("{code}:{message}"),
            primary,
        ),
        code: code.to_owned(),
        category: DiagnosticCategory::Analysis,
        severity: Severity::Error,
        message,
        primary,
        related: Vec::new(),
        tags: Vec::new(),
    }
}

fn validate(input: &RuleInput<'_, '_>, options: &Options) -> io::Result<()> {
    input.source.validate()?;
    input.settings.validate()?;

    if input.bindings.source != input.source.version
        || input.directives.source != input.source.version
    {
        return Err(invalid("lint lexical or directive snapshot mismatch"));
    }

    if input.syntax.snapshot().version != input.source.version
        || input.syntax.snapshot().text != input.source.text
    {
        return Err(invalid("lint syntax snapshot mismatch"));
    }

    for directive in &input.directives.directives {
        options.check()?;
        directive.location.validate(input.source)?;
        directive.scope.validate(input.source)?;
    }

    for dependency in input.imports {
        options.check()?;
        dependency.call.validate(input.source)?;
        dependency.argument.validate(input.source)?;

        if !dependency.constant && matches!(dependency.requested, Knowledge::Known(Some(_))) {
            return Err(invalid("nonconstant import has a static request"));
        }

        if dependency.argument.span.start < dependency.call.span.start
            || dependency.argument.span.end > dependency.call.span.end
        {
            return Err(invalid("lint import argument is outside its call"));
        }
    }

    if let Some(facts) = input.facts {
        if facts.analysis.module.source != input.source.version.identity {
            return Err(invalid("lint semantic source identity mismatch"));
        }

        for location in facts
            .facts
            .occurrences
            .iter()
            .map(|fact| &fact.location)
            .chain(facts.facts.expressions.iter().map(|fact| &fact.location))
            .chain(facts.facts.calls.iter().map(|fact| &fact.location))
        {
            options.check()?;
            location.validate(input.source)?;
        }

        for binding in &facts.facts.bindings {
            options.check()?;

            if let Knowledge::Known(Some(location)) = &binding.declaration
                && location.source.identity == input.source.version.identity
            {
                location.validate(input.source)?;
            }
        }
    }

    Ok(())
}

fn report(options: &Options, source: &SourceSnapshot, completed: usize, total: usize) {
    options.report(Event {
        invocation: 0,
        operation: 0,
        payload: EventPayload::Progress(Progress {
            phase: "Collecting lint findings".to_owned(),
            source: Some(source.version),
            completed,
            total: Some(total),
        }),
    });
}
