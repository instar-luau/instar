use std::{
    collections::BTreeSet,
    io, mem,
    sync::{Arc, Mutex},
};

use instar_luau::{
    AnalysisPurpose, AnalysisRequest, ImportOutcome, InspectionResult, ModuleIdentity,
    SemanticAvailability, SemanticCategory, SemanticIdentity,
};

use instar_shared::{
    Availability, Completion, Diagnostic, FailureKind, Knowledge, Location, OperationFailure,
    Options, Reason, SourceIdentity, SourceOutcome, SourceSnapshot, SourceVersion, Span,
};

use instar_syntax::{parse, token::TokenKind};
use ureq::http::Uri;

use super::{
    assistance::assistance,
    hierarchy::hierarchy,
    index::Index,
    navigation,
    output::{ProjectCandidate, ProjectCompletions, ProjectSymbol, QueryOutput},
    presentation::presentation,
    request::{
        CompletionQuery, DiagnosticQuery, HierarchyQuery, HierarchyQueryKind, LinkQuery,
        PositionQuery, PositionQueryKind, PresentationKind, PresentationQuery, Query, QueryRequest,
        ReferenceQuery, SymbolQuery,
    },
};

use crate::{
    assets,
    capture::Capture,
    configuration::{
        CheckOverrides, DiagnosticScope, LintOverrides, OperationOverrides, SelectionOverrides,
        worker_options,
    },
    documentation,
    execution::{Runtime, Work, WorkOutput, failure},
    graph::{ClosureRecipe, DependencyGraph, syntax_settings},
    inputs::{ProjectSnapshot, SourceOrigin},
    report::{ProjectLink, QueryReport},
    request::{CheckRequest, FixMode, Input, Invocation, LintRequest},
    selection::{Matcher, SelectedSource, SelectionPurpose},
    symbols,
};

pub(crate) enum QueryEvidence<'categories> {
    Position(&'categories [SemanticCategory]),
    Inventory(&'categories [SemanticCategory]),
}

pub(crate) struct QueryCapture {
    pub(crate) capture: Capture,
    pub(crate) graph: DependencyGraph,
    pub(crate) recipes: Vec<ClosureRecipe>,
    pub(crate) index: Index,
    pub(crate) completion: Completion,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) failures: Vec<OperationFailure>,
    pub(crate) outcomes: Vec<SourceOutcome>,
}

impl QueryCapture {
    pub(crate) fn require_complete(&self, options: &Options) -> io::Result<()> {
        if !self.failures.is_empty() {
            return Err(failed(
                None,
                self.failures.clone(),
                self.completion.clone(),
                self.diagnostics.clone(),
                self.outcomes.clone(),
                options,
            ));
        }

        if self.completion != Completion::Complete {
            return Err(failed(
                Some(self.completion.error()),
                Vec::new(),
                self.completion.clone(),
                self.diagnostics.clone(),
                self.outcomes.clone(),
                options,
            ));
        }

        Ok(())
    }
}

struct QueryState {
    completion: Completion,
    diagnostics: Vec<Diagnostic>,
    failures: Vec<OperationFailure>,
    outcomes: Vec<SourceOutcome>,
    availability: Availability,
}

impl QueryState {
    fn new() -> Self {
        Self {
            completion: Completion::Complete,
            diagnostics: Vec::new(),
            failures: Vec::new(),
            outcomes: Vec::new(),
            availability: Availability::Available,
        }
    }

    fn include_capture(&mut self, capture: &QueryCapture) {
        self.failures.extend(capture.failures.clone());
        self.outcomes.extend(capture.outcomes.clone());
        self.completion.clone_from(&capture.completion);
        self.diagnostics.clone_from(&capture.diagnostics);
    }

    fn include_inspection(&mut self, inspection: &InspectionResult) {
        self.completion = self
            .completion
            .clone()
            .combine(inspection.completion.clone());

        self.diagnostics.extend(inspection.diagnostics.clone());
    }

    fn error(self, error: Option<io::Error>, options: &Options) -> io::Error {
        failed(
            error,
            self.failures,
            self.completion,
            self.diagnostics,
            self.outcomes,
            options,
        )
    }
}

impl Runtime {
    pub(crate) fn query(
        &mut self,
        request: QueryRequest,
        options: &Options,
    ) -> io::Result<QueryReport> {
        options.check()?;

        let workspace = match &request.operation {
            Query::Symbols(request) => request.source.is_none(),
            Query::References(request) => request.workspace,
            Query::Hierarchy(_) => true,
            Query::Complete(request) => !request.inline,

            Query::Inspect(request) => matches!(
                request.kind,
                PositionQueryKind::Implementation | PositionQueryKind::Lenses
            ),

            Query::Diagnostics(request) => {
                request.inputs.is_empty()
                    || request
                        .inputs
                        .iter()
                        .any(|input| matches!(input, Input::Path(_) | Input::Pattern { .. }))
            }

            _ => false,
        };

        let capture = Capture::new(Arc::clone(&self.host), request.snapshot);
        let mut state = QueryState::new();
        let result = self.query_output(request.operation, capture, &mut state, options);

        let (output, mut capture) = match result {
            Ok(output) if state.failures.is_empty() => output,
            result => return Err(state.error(result.err(), options)),
        };

        if state.completion != Completion::Complete && state.availability == Availability::Available
        {
            state.availability = Availability::Partial(match &state.completion {
                Completion::Incomplete(reasons) => reasons
                    .first()
                    .copied()
                    .unwrap_or(Reason::FailedPrerequisite),

                _ => Reason::FailedPrerequisite,
            });
        }

        if workspace {
            capture.admit_inventory(options)?;
            capture.consult_inventory();
        }

        Ok(QueryReport {
            snapshot: capture.snapshot.clone(),
            inputs: capture.guards(),
            output,
            availability: state.availability,
            diagnostics: state.diagnostics,
            completion: state.completion,
        })
    }

    pub(crate) fn editor_targets(
        &mut self,
        source: Option<SourceIdentity>,
        workspace: bool,
        capture: &mut Capture,
        context: Option<&CheckRequest>,
        options: &Options,
    ) -> io::Result<Vec<SelectedSource>> {
        let invocation = Invocation {
            inputs: if workspace {
                Vec::new()
            } else {
                source.map_or_else(Vec::new, |identity| vec![Input::Source(identity)])
            },
            ..context.map_or_else(Invocation::default, |context| context.invocation.clone())
        };

        let overrides = OperationOverrides::Check(CheckOverrides {
            selection: SelectionOverrides {
                include: Some(Vec::new()),
                exclude: Some(Vec::new()),
                override_exclusions: true,
            },
            ..context.map_or_else(CheckOverrides::default, |context| context.overrides.clone())
        });

        let selected = self
            .select(
                &invocation,
                &overrides,
                capture,
                SelectionPurpose::Editor,
                options,
            )?
            .targets;

        let mut targets = Vec::new();

        for target in selected {
            if Some(target.identity) == source || indexed(&target, &capture.snapshot)? {
                targets.push(target);
            }
        }

        if workspace {
            let documents = capture
                .snapshot
                .sources()
                .filter(|source| {
                    matches!(
                        capture.snapshot.origin(source.version.identity),
                        Some(SourceOrigin::Document { .. })
                    )
                })
                .cloned()
                .collect::<Vec<_>>();

            for document in documents {
                let identity = document.version.identity;

                if capture
                    .snapshot
                    .origin(identity)
                    .and_then(SourceOrigin::path)
                    .is_some_and(|path| {
                        path.file_name().is_some_and(|name| name == "instar.luau")
                            || !matches!(
                                path.extension().and_then(|extension| extension.to_str()),
                                Some("lua" | "luau")
                            )
                    })
                {
                    continue;
                }

                if targets.iter().any(|target| target.identity == identity) {
                    continue;
                }

                let effective =
                    self.effective(identity, &invocation, &overrides, capture, options)?;

                let target = SelectedSource {
                    identity,
                    effective,
                };

                if Some(identity) == source || indexed(&target, &capture.snapshot)? {
                    capture.consult_source(&document);
                    targets.push(target);
                }
            }
        }

        if let Some(identity) = source
            && !targets.iter().any(|target| target.identity == identity)
        {
            let effective = self.effective(identity, &invocation, &overrides, capture, options)?;

            targets.push(SelectedSource {
                identity,
                effective,
            });
        }

        Ok(targets)
    }

    pub(crate) fn query_capture(
        &mut self,
        mut capture: Capture,
        source: Option<SourceIdentity>,
        workspace: bool,
        evidence: QueryEvidence<'_>,
        context: Option<&CheckRequest>,
        options: &Options,
    ) -> io::Result<QueryCapture> {
        let (categories, inventory) = match evidence {
            QueryEvidence::Position(categories) => (categories, false),
            QueryEvidence::Inventory(categories) => (categories, true),
        };

        let selected = self.editor_targets(source, workspace, &mut capture, context, options)?;

        let invocation =
            context.map_or_else(Invocation::default, |context| context.invocation.clone());

        let overrides = OperationOverrides::Check(
            context.map_or_else(CheckOverrides::default, |context| context.overrides.clone()),
        );

        let graph = self.graph(
            &selected,
            &invocation,
            &overrides,
            &mut capture,
            true,
            options,
        )?;

        let failures = graph
            .modules
            .values()
            .flat_map(|node| node.environment.failures.iter().cloned())
            .collect::<Vec<_>>();

        let (recipes, completion, findings) = self
            .closures(
                &graph,
                AnalysisPurpose::Editor,
                categories,
                &capture,
                options,
            )
            .map_err(|error| {
                failed(
                    Some(error),
                    failures.clone(),
                    graph.completion.clone(),
                    graph
                        .findings
                        .iter()
                        .map(|finding| finding.diagnostic.clone())
                        .collect(),
                    Vec::new(),
                    options,
                )
            })?;

        let mut diagnostics = findings
            .into_iter()
            .map(|finding| finding.diagnostic)
            .collect::<Vec<_>>();

        diagnostics.extend(
            graph
                .findings
                .iter()
                .map(|finding| finding.diagnostic.clone()),
        );

        let mut state = QueryCapture {
            capture,
            graph,
            recipes,
            index: Index::default(),
            completion,
            diagnostics,
            failures,
            outcomes: Vec::new(),
        };

        self.query_analysis(&mut state, options)?;

        if inventory {
            self.query_facts(&mut state, categories, options)?;
        }

        if workspace {
            state.capture.admit_inventory(options)?;
            state.capture.consult_inventory();
        }

        Ok(state)
    }

    fn query_output(
        &mut self,
        operation: Query,
        mut capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        match operation {
            Query::Content(identity) => {
                let source = capture.snapshot.source(identity).cloned();

                if let Some(source) = &source {
                    capture.consult_source(source);
                }

                Ok((QueryOutput::Content(source), capture))
            }

            Query::Diagnostics(request) => self.query_diagnostics(request, capture, state, options),
            Query::Symbols(request) => self.query_symbols(&request, capture, options),
            Query::Links(request) => self.query_links(&request, capture, state, options),

            Query::Documentation { source, symbol } => {
                self.query_documentation(source, &symbol, capture, options)
            }

            Query::Presentation(request) => {
                self.query_presentation(&request, capture, state, options)
            }

            Query::Inspect(request) => self.query_inspection(&request, capture, state, options),
            Query::References(request) => self.query_references(&request, capture, state, options),
            Query::Hierarchy(request) => self.query_hierarchy(&request, capture, state, options),
            Query::Complete(request) => self.query_completions(&request, capture, state, options),
        }
    }

    fn query_diagnostics(
        &mut self,
        request: DiagnosticQuery,
        mut capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        let mut invocation = Invocation {
            inputs: request.inputs,
            ..Invocation::default()
        };

        let selected = self.select(
            &invocation,
            &OperationOverrides::Check(CheckOverrides {
                selection: SelectionOverrides {
                    include: Some(Vec::new()),
                    exclude: Some(Vec::new()),
                    override_exclusions: true,
                },
                ..CheckOverrides::default()
            }),
            &mut capture,
            SelectionPurpose::Editor,
            options,
        )?;

        let mut inputs = Vec::new();

        for target in selected.targets {
            let explicit = invocation.inputs.iter().any(|input| match input {
                Input::Source(identity) => *identity == target.identity,
                Input::Module(module) => module.source == target.identity,
                _ => false,
            });

            let open = matches!(
                capture.snapshot.origin(target.identity),
                Some(SourceOrigin::Document { .. })
            );

            if explicit
                || open
                || matches!(
                    target.effective.settings.editing.diagnostic_scope,
                    DiagnosticScope::Workspace
                ) && indexed(&target, &capture.snapshot)?
            {
                inputs.push(Input::Source(target.identity));
            }
        }

        invocation.inputs = inputs;
        let selected = !invocation.inputs.is_empty();

        let check = if request.checking && selected {
            match self.check(
                &CheckRequest {
                    invocation: invocation.clone(),
                    overrides: CheckOverrides::default(),
                },
                Some(capture.snapshot.clone()),
                options,
            ) {
                Ok(report) => Some(Box::new(report)),

                Err(error) if deferred(&error) => {
                    state
                        .failures
                        .push(failure(&error, options, &Mutex::default()));

                    None
                }

                Err(error) => return Err(error),
            }
        } else {
            None
        };

        if let Some(report) = &check {
            state.completion = state.completion.clone().combine(report.completion.clone());
            state.diagnostics.extend(report.findings.clone());
            capture.snapshot = report.snapshot.clone();

            for input in &report.inputs {
                capture.inputs.insert(input.identity.clone(), input.clone());
            }
        }

        let lint = if request.lint && selected {
            match self.lint(
                &LintRequest {
                    invocation,
                    overrides: LintOverrides::default(),
                    fixes: FixMode::None,
                    fix_rules: None,
                },
                Some(capture.snapshot.clone()),
                options,
            ) {
                Ok(report) => Some(Box::new(report)),

                Err(error) if deferred(&error) => {
                    state
                        .failures
                        .push(failure(&error, options, &Mutex::default()));

                    None
                }

                Err(error) => return Err(error),
            }
        } else {
            None
        };

        if let Some(report) = &lint {
            state.completion = state.completion.clone().combine(report.completion.clone());
            state.diagnostics.extend(report.findings.clone());
            state.outcomes.extend(report.outcomes.clone());
            capture.snapshot = report.snapshot.clone();

            for input in &report.inputs {
                capture.inputs.insert(input.identity.clone(), input.clone());
            }
        }

        Ok((QueryOutput::Diagnostics { check, lint }, capture))
    }

    fn query_symbols(
        &mut self,
        request: &SymbolQuery,
        mut capture: Capture,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        let selected = self.editor_targets(
            request.source,
            request.source.is_none(),
            &mut capture,
            None,
            options,
        )?;

        let mut output = Vec::new();

        for target in selected {
            let source = capture
                .snapshot
                .source(target.identity)
                .ok_or_else(|| invalid("symbol source is absent"))?;

            let symbols = symbols::symbols(source, &syntax_settings(&target.effective), options)?;

            let mut pending = symbols
                .into_iter()
                .rev()
                .map(|symbol| (symbol, None))
                .collect::<Vec<_>>();

            while let Some((mut symbol, container)) = pending.pop() {
                options.check()?;

                if request.source.is_none() {
                    pending.extend(
                        mem::take(&mut symbol.children)
                            .into_iter()
                            .rev()
                            .map(|child| (child, Some(symbol.name.clone()))),
                    );
                }

                if request.search.is_empty()
                    || symbol
                        .name
                        .to_lowercase()
                        .contains(&request.search.to_lowercase())
                {
                    let range = if request.source.is_some() {
                        symbol.range
                    } else {
                        symbol.selection
                    };

                    output.push(ProjectSymbol {
                        location: Location::new(
                            source,
                            Span {
                                start: range[0],
                                end: range[1],
                            },
                        )?,
                        symbol,
                        container,
                    });
                }
            }
        }

        Ok((QueryOutput::Symbols(output), capture))
    }

    fn query_links(
        &mut self,
        request: &LinkQuery,
        mut capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        checked_source(&capture.snapshot, request.source)?;

        let selected = self.editor_targets(
            Some(request.source.identity),
            false,
            &mut capture,
            None,
            options,
        )?;

        let graph = self.graph(
            &selected,
            &Invocation::default(),
            &OperationOverrides::Check(CheckOverrides::default()),
            &mut capture,
            false,
            options,
        )?;

        state.failures.extend(
            graph
                .modules
                .values()
                .flat_map(|node| node.environment.failures.iter().cloned()),
        );

        state.completion = graph.completion;

        state.diagnostics = graph
            .findings
            .into_iter()
            .map(|finding| finding.diagnostic)
            .collect();

        let mut links = Vec::new();

        for node in graph
            .modules
            .values()
            .filter(|node| node.source.version == request.source)
        {
            for import in &node.imports {
                let target = if let ImportOutcome::Module(module) = import.outcome {
                    Some(module.source)
                } else {
                    None
                };

                let address = target
                    .and_then(|target| capture.snapshot.origin(target).and_then(SourceOrigin::path))
                    .map(|path| path.to_string_lossy().into_owned());

                links.push(ProjectLink {
                    location: import.site.argument,
                    target,
                    address,
                });
            }
        }

        links.sort_by_key(|link| (link.location.source, link.location.span));
        let mut agreed = Vec::new();

        for group in links.chunk_by(|left, right| left.location == right.location) {
            let link = &group[0];

            if link.target.is_some()
                && group
                    .iter()
                    .all(|candidate| candidate.target == link.target)
            {
                agreed.push(link.clone());
            }
        }

        let source = checked_source(&capture.snapshot, request.source)?;

        let target = selected
            .iter()
            .find(|target| target.identity == request.source.identity)
            .ok_or_else(|| invalid("link source is absent"))?;

        let parsed = parse(source, &syntax_settings(&target.effective), options);
        state.completion = state.completion.clone().combine(parsed.completion);
        state.diagnostics.extend(parsed.diagnostics);

        for token in &parsed.tree.syntax().tokens {
            options.check()?;

            if !matches!(token.kind, TokenKind::Comment | TokenKind::BlockComment) {
                continue;
            }

            let text = &source.text[token.span.start..token.span.end];
            let mut covered = 0;

            for (offset, character) in text.char_indices() {
                options.check()?;

                if offset < covered
                    || !matches!(character, 'h' | 'H')
                    || text[..offset]
                        .chars()
                        .next_back()
                        .is_some_and(|previous| previous.is_alphanumeric() || previous == '_')
                {
                    continue;
                }

                let remaining = &text[offset..];

                let scheme = if remaining
                    .get(..8)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
                {
                    "https://"
                } else if remaining
                    .get(..7)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
                {
                    "http://"
                } else {
                    continue;
                };

                let mut brackets = 0;

                let end = remaining
                    .char_indices()
                    .find_map(|(index, character)| {
                        if character == '[' {
                            brackets += 1;
                        } else if character == ']' {
                            if brackets == 0 {
                                return Some(index);
                            }

                            brackets -= 1;
                        }

                        (character.is_whitespace()
                            || matches!(character, '<' | '>' | '"' | '\'' | '`' | '{' | '}'))
                        .then_some(index)
                    })
                    .unwrap_or(remaining.len());

                let mut address = &remaining[..end];

                let opening = address
                    .chars()
                    .filter(|character| *character == '(')
                    .count();

                let mut closing = address
                    .chars()
                    .filter(|character| *character == ')')
                    .count();

                while address.ends_with(')') && closing > opening {
                    address = &address[..address.len() - 1];
                    closing -= 1;
                }

                let normalized = format!("{scheme}{}", &address[scheme.len()..]);
                covered = offset + address.len();

                if normalized
                    .parse::<Uri>()
                    .ok()
                    .and_then(|address| address.host().map(str::to_owned))
                    .is_none_or(|host| host.is_empty())
                    || normalized.match_indices('%').any(|(offset, _)| {
                        normalized.get(offset + 1..offset + 3).is_none_or(|digits| {
                            !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    })
                {
                    continue;
                }

                agreed.push(ProjectLink {
                    location: Location::new(
                        source,
                        Span {
                            start: token.span.start + offset,
                            end: token.span.start + covered,
                        },
                    )?,
                    target: None,
                    address: Some(normalized),
                });
            }
        }

        agreed.sort_by_key(|link| (link.location.source, link.location.span));

        agreed.dedup_by(|left, right| {
            left.location == right.location
                && left.target == right.target
                && left.address == right.address
        });

        Ok((QueryOutput::Links(agreed), capture))
    }

    fn query_documentation(
        &mut self,
        source: SourceVersion,
        symbol: &str,
        mut capture: Capture,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        checked_source(&capture.snapshot, source)?;

        let target = self
            .editor_targets(Some(source.identity), false, &mut capture, None, options)?
            .pop()
            .ok_or_else(|| invalid("documentation source is absent"))?;

        let mut index = documentation::Index::default();

        if symbol.starts_with("@luau/") {
            index.merge(documentation::load(
                &assets::documentation_cache_directory()?,
                &mut capture,
                options,
            )?)?;
        }

        if target.effective.settings.environment.roblox {
            let directory = target
                .effective
                .settings
                .environment
                .asset_directory
                .clone()
                .map_or_else(assets::roblox_cache_directory, Ok)?;

            index.merge(
                assets::load(
                    &directory,
                    target.effective.settings.environment.security,
                    &mut capture,
                    options,
                )?
                .documentation,
            )?;
        }

        for group in &target.effective.settings.environment.groups {
            for path in &group.documentation {
                let source = capture.source(path, options)?;

                index.merge(documentation::Index::parse(
                    &source.text,
                    &[&group.namespace],
                )?)?;
            }
        }

        Ok((
            QueryOutput::Documentation(index.get(symbol).cloned()),
            capture,
        ))
    }

    fn query_presentation(
        &mut self,
        request: &PresentationQuery,
        mut capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        let source = checked_source(&capture.snapshot, request.source)?.clone();

        if matches!(request.kind, PresentationKind::Tokens { .. }) {
            let captured = self.query_capture(
                capture,
                Some(source.version.identity),
                false,
                QueryEvidence::Inventory(&[
                    SemanticCategory::Bindings,
                    SemanticCategory::Occurrences,
                ]),
                None,
                options,
            )?;

            state.include_capture(&captured);

            let node = captured
                .graph
                .modules
                .values()
                .find(|node| node.source.version == source.version)
                .ok_or_else(|| invalid("presentation source is absent"))?;

            let facts = captured
                .index
                .analyses
                .values()
                .find(|facts| facts.analysis.module == node.identity);

            let output = presentation(
                request,
                &source,
                &syntax_settings(&node.effective),
                facts,
                node.effective.settings.environment.roblox,
                options,
            )?;

            Ok((QueryOutput::Presentation(output), captured.capture))
        } else {
            let target = self
                .editor_targets(
                    Some(source.version.identity),
                    false,
                    &mut capture,
                    None,
                    options,
                )?
                .pop()
                .ok_or_else(|| invalid("presentation source is absent"))?;

            let output = presentation(
                request,
                &source,
                &syntax_settings(&target.effective),
                None,
                target.effective.settings.environment.roblox,
                options,
            )?;

            Ok((QueryOutput::Presentation(output), capture))
        }
    }

    fn query_inspection(
        &mut self,
        request: &PositionQuery,
        capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        checked_position(&capture.snapshot, request)?;
        let categories = categories(request.kind);

        let workspace = matches!(
            request.kind,
            PositionQueryKind::Implementation | PositionQueryKind::Lenses
        );

        let evidence = if matches!(
            request.kind,
            PositionQueryKind::Implementation
                | PositionQueryKind::Hints
                | PositionQueryKind::Lenses
                | PositionQueryKind::InlineValues
        ) {
            QueryEvidence::Inventory(&categories)
        } else {
            QueryEvidence::Position(&categories)
        };

        let captured = self.query_capture(
            capture,
            Some(request.source.identity),
            workspace,
            evidence,
            None,
            options,
        )?;

        state.include_capture(&captured);

        if matches!(
            request.kind,
            PositionQueryKind::Declaration
                | PositionQueryKind::Definition
                | PositionQueryKind::TypeDefinition
                | PositionQueryKind::Implementation
        ) && request.module.context == captured.capture.snapshot.context(request.source.identity)
        {
            let mut modules = BTreeSet::new();
            let mut locations = Vec::new();

            for recipe in &captured.recipes {
                for module in &recipe.request.modules {
                    if module.analysis.module.source != request.source.identity
                        || !modules.insert(module.analysis.module)
                    {
                        continue;
                    }

                    let WorkOutput::Inspection(inspection) = self.workers.execute(
                        Work::Inspect {
                            closure: recipe.request.clone(),
                            module: module.analysis.module,
                            offset: request.offset,
                        },
                        &worker_options(options),
                    )?
                    else {
                        return Err(io::Error::other("inspection lane returned wrong outcome"));
                    };

                    state.include_inspection(&inspection);

                    locations.extend(
                        navigation::locations(&inspection, request.kind, &captured.index)
                            .map_err(Reason::error)?,
                    );
                }
            }

            if modules.is_empty() {
                return Err(invalid("source has unavailable semantic contexts"));
            }

            locations.sort_by_key(|location| (location.source, location.span));
            locations.dedup();

            return Ok((QueryOutput::Locations(locations), captured.capture));
        }

        let output = if matches!(
            request.kind,
            PositionQueryKind::Hints | PositionQueryKind::Lenses | PositionQueryKind::InlineValues
        ) {
            QueryOutput::Assistance(
                assistance(request, &captured.capture.snapshot, &captured.index)
                    .map_err(Reason::error)?,
            )
        } else {
            let (closure, module) = selected_recipe(&captured, request.module)?;

            let WorkOutput::Inspection(inspection) = self.workers.execute(
                Work::Inspect {
                    closure,
                    module,
                    offset: request.offset,
                },
                &worker_options(options),
            )?
            else {
                return Err(io::Error::other("inspection lane returned wrong outcome"));
            };

            state.include_inspection(&inspection);

            if matches!(request.kind, PositionQueryKind::PrepareRename) {
                let location = inspection
                    .occurrence
                    .as_ref()
                    .map(|occurrence| match &occurrence.targets {
                        Knowledge::Known(targets) => Ok((occurrence, targets)),
                        Knowledge::Unavailable(reason) => Err(reason.error()),
                    })
                    .transpose()?
                    .filter(|(_, targets)| {
                        matches!(targets.as_slice(), [SemanticIdentity::Source { location, .. }]
                        if !captured.capture.snapshot.is_declaration(location.source)
                            && !matches!(
                                captured.capture.snapshot.origin(location.source.identity),
                                Some(SourceOrigin::Virtual { .. })
                            ))
                    })
                    .map(|(occurrence, _)| occurrence.location);

                QueryOutput::Locations(location.into_iter().collect())
            } else if matches!(
                request.kind,
                PositionQueryKind::Declaration
                    | PositionQueryKind::Definition
                    | PositionQueryKind::TypeDefinition
                    | PositionQueryKind::Implementation
            ) {
                QueryOutput::Locations(
                    navigation::locations(&inspection, request.kind, &captured.index)
                        .map_err(Reason::error)?,
                )
            } else {
                QueryOutput::Inspection(inspection)
            }
        };

        Ok((output, captured.capture))
    }

    fn query_references(
        &mut self,
        request: &ReferenceQuery,
        capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        checked_position(&capture.snapshot, &request.position)?;

        let captured = self.query_capture(
            capture,
            Some(request.position.source.identity),
            request.workspace,
            QueryEvidence::Inventory(&[SemanticCategory::Bindings, SemanticCategory::Occurrences]),
            None,
            options,
        )?;

        state.include_capture(&captured);

        if !captured
            .index
            .available(&[SemanticCategory::Bindings, SemanticCategory::Occurrences])
        {
            state.availability = Availability::Unavailable(Reason::UnavailableSemantics);
        }

        let output = navigation::references(request, &captured.index).map_err(Reason::error)?;

        Ok((QueryOutput::References(output), captured.capture))
    }

    fn query_hierarchy(
        &mut self,
        request: &HierarchyQuery,
        capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        let source = request
            .position
            .as_ref()
            .map(|position| position.source.identity)
            .or_else(|| {
                request.identity.as_ref().and_then(|identity| {
                    if let SemanticIdentity::Source { location, .. } = identity {
                        (!capture.snapshot.is_declaration(location.source))
                            .then_some(location.source.identity)
                    } else {
                        None
                    }
                })
            });

        if let Some(position) = &request.position {
            checked_position(&capture.snapshot, position)?;
        }

        let categories = match request.kind {
            HierarchyQueryKind::PrepareCalls => vec![
                SemanticCategory::Bindings,
                SemanticCategory::Occurrences,
                SemanticCategory::Functions,
            ],

            HierarchyQueryKind::IncomingCalls | HierarchyQueryKind::OutgoingCalls => {
                vec![SemanticCategory::Functions, SemanticCategory::Calls]
            }

            HierarchyQueryKind::PrepareTypes => {
                vec![SemanticCategory::Types, SemanticCategory::Nominals]
            }

            HierarchyQueryKind::Supertypes | HierarchyQueryKind::Subtypes => {
                vec![SemanticCategory::Nominals, SemanticCategory::Inheritance]
            }
        };

        let captured = self.query_capture(
            capture,
            source,
            true,
            QueryEvidence::Inventory(&categories),
            None,
            options,
        )?;

        state.include_capture(&captured);

        let inspection = if matches!(request.kind, HierarchyQueryKind::PrepareTypes)
            && let Some(position) = &request.position
        {
            let (closure, module) = selected_recipe(&captured, position.module)?;

            let WorkOutput::Inspection(inspection) = self.workers.execute(
                Work::Inspect {
                    closure,
                    module,
                    offset: position.offset,
                },
                &worker_options(options),
            )?
            else {
                return Err(io::Error::other("inspection lane returned wrong outcome"));
            };

            state.include_inspection(&inspection);

            Some(inspection)
        } else {
            None
        };

        let output =
            hierarchy(request, &captured.index, inspection.as_ref()).map_err(Reason::error)?;

        Ok((QueryOutput::Hierarchy(output), captured.capture))
    }

    fn query_completions(
        &mut self,
        request: &CompletionQuery,
        capture: Capture,
        state: &mut QueryState,
        options: &Options,
    ) -> io::Result<(QueryOutput, Capture)> {
        checked_position(&capture.snapshot, &request.position)?;

        let captured = self.query_capture(
            capture,
            Some(request.position.source.identity),
            false,
            QueryEvidence::Inventory(&[
                SemanticCategory::Bindings,
                SemanticCategory::Occurrences,
                SemanticCategory::Types,
            ]),
            None,
            options,
        )?;

        state.include_capture(&captured);

        if !captured
            .graph
            .modules
            .contains_key(&request.position.module)
            && captured
                .graph
                .modules
                .keys()
                .filter(|module| module.source == request.position.source.identity)
                .count()
                > 1
        {
            return Err(invalid("completion source has multiple module contexts"));
        }

        let (closure, module) = selected_recipe(&captured, request.position.module)?;

        let WorkOutput::Completions(native) = self.workers.execute(
            Work::Complete {
                closure,
                module,
                offset: request.position.offset,
            },
            &worker_options(options),
        )?
        else {
            return Err(io::Error::other("completion lane returned wrong outcome"));
        };

        let mut capture = captured.capture;

        let mut completions = ProjectCompletions {
            candidates: native
                .candidates
                .iter()
                .cloned()
                .map(|entry| ProjectCandidate {
                    entry,
                    edits: Vec::new(),
                })
                .collect(),
            native,
        };

        let imports = self.complete_imports(
            request,
            &captured.graph,
            &mut capture,
            &mut completions,
            &mut state.failures,
            options,
        );

        state.completion = state
            .completion
            .clone()
            .combine(completions.native.completion.clone());

        state
            .diagnostics
            .extend(completions.native.diagnostics.clone());

        imports?;

        Ok((QueryOutput::Completions(completions), capture))
    }

    fn query_analysis(&mut self, state: &mut QueryCapture, options: &Options) -> io::Result<()> {
        let mut analysis_failed = false;

        for result in self.analyze(&state.recipes, options) {
            match result {
                Ok(result) => {
                    state.completion = state.completion.clone().combine(result.completion.clone());

                    for module in &result.modules {
                        let mut completion = module.completion.clone().combine(
                            state.graph.modules[&module.analysis.module]
                                .environment
                                .completion
                                .clone(),
                        );

                        state.diagnostics.extend(module.parsed.diagnostics.clone());
                        state.diagnostics.extend(module.diagnostics.clone());

                        if let Some(environment) = &module.environment {
                            completion = completion.combine(environment.completion.clone());
                            state.diagnostics.extend(environment.diagnostics.clone());
                        }

                        state.completion = state.completion.clone().combine(completion.clone());

                        state.outcomes.push(SourceOutcome {
                            source: module.parsed.input.source.version,
                            completion,
                        });
                    }
                }

                Err(error) => {
                    state
                        .failures
                        .push(failure(&error, options, &Mutex::default()));

                    analysis_failed = true;
                }
            }
        }

        if analysis_failed {
            return Err(failed(
                None,
                state.failures.clone(),
                state.completion.clone(),
                state.diagnostics.clone(),
                state.outcomes.clone(),
                options,
            ));
        }

        Ok(())
    }

    fn query_facts(
        &mut self,
        state: &mut QueryCapture,
        categories: &[SemanticCategory],
        options: &Options,
    ) -> io::Result<()> {
        let work = state
            .recipes
            .iter()
            .flat_map(|recipe| {
                recipe.request.modules.iter().map(|module| Work::Facts {
                    closure: recipe.request.clone(),
                    module: module.analysis.module,
                    categories: categories.to_vec(),
                })
            })
            .collect();

        let mut semantics_failed = false;

        for result in self.workers.batch(work, &worker_options(options)) {
            match result {
                Ok(WorkOutput::Facts(facts)) => {
                    state.completion = state.completion.clone().combine(requested_completion(
                        facts.completion.clone(),
                        &facts.availability,
                        categories,
                    ));

                    state.diagnostics.extend(facts.diagnostics.clone());
                    state.index.replace(facts);
                }

                Ok(_) => {
                    state.failures.push(failure(
                        &io::Error::other("semantic lane returned wrong outcome"),
                        options,
                        &Mutex::default(),
                    ));

                    semantics_failed = true;
                }

                Err(error) => {
                    state
                        .failures
                        .push(failure(&error, options, &Mutex::default()));

                    semantics_failed = true;
                }
            }
        }

        if semantics_failed {
            return Err(failed(
                None,
                state.failures.clone(),
                state.completion.clone(),
                state.diagnostics.clone(),
                state.outcomes.clone(),
                options,
            ));
        }

        Ok(())
    }
}

pub(crate) fn selected_recipe(
    state: &QueryCapture,
    requested: ModuleIdentity,
) -> io::Result<(AnalysisRequest, ModuleIdentity)> {
    let matches = state
        .recipes
        .iter()
        .flat_map(|recipe| {
            recipe
                .request
                .modules
                .iter()
                .filter(|module| module.analysis.module.source == requested.source)
                .map(|module| (&recipe.request, module.analysis.module))
        })
        .collect::<Vec<_>>();

    let (recipe, module) = matches
        .iter()
        .find(|(_, module)| *module == requested)
        .copied()
        .or_else(|| (matches.len() == 1).then(|| matches[0]))
        .ok_or_else(|| {
            if matches.len() > 1 {
                OperationFailure {
                    kind: FailureKind::Operational,
                    message: "source has ambiguous semantic contexts".to_owned(),
                    completion: Completion::Incomplete(vec![Reason::UnavailableSemantics]),
                    diagnostics: Vec::new(),
                    completed_sources: Vec::new(),
                    completed_edits: Vec::new(),
                }
                .into()
            } else {
                Reason::UnavailableSemantics.error()
            }
        })?;

    Ok((recipe.clone(), module))
}

pub(crate) fn checked_source(
    snapshot: &ProjectSnapshot,
    version: SourceVersion,
) -> io::Result<&SourceSnapshot> {
    snapshot
        .source(version.identity)
        .filter(|source| source.version == version)
        .ok_or_else(|| Reason::StaleQuery.error())
}

pub(crate) fn checked_position(
    snapshot: &ProjectSnapshot,
    position: &PositionQuery,
) -> io::Result<()> {
    let source = checked_source(snapshot, position.source)?;

    if position.module.source != position.source.identity
        || !source.text.is_char_boundary(position.offset)
    {
        return Err(invalid("query position does not match its source revision"));
    }

    Ok(())
}

fn categories(kind: PositionQueryKind) -> Vec<SemanticCategory> {
    let mut categories = vec![SemanticCategory::Bindings, SemanticCategory::Occurrences];

    match kind {
        PositionQueryKind::Hover | PositionQueryKind::Hints | PositionQueryKind::TypeDefinition => {
            categories.push(SemanticCategory::Types);
        }

        PositionQueryKind::Signatures => {
            categories.push(SemanticCategory::Calls);
        }

        PositionQueryKind::Implementation => {
            categories.extend([
                SemanticCategory::Functions,
                SemanticCategory::Nominals,
                SemanticCategory::Inheritance,
            ]);
        }

        PositionQueryKind::Lenses => categories.extend([
            SemanticCategory::Functions,
            SemanticCategory::Calls,
            SemanticCategory::Nominals,
            SemanticCategory::Inheritance,
        ]),

        _ => {}
    }

    categories
}

fn requested_completion(
    mut completion: Completion,
    availability: &SemanticAvailability,
    categories: &[SemanticCategory],
) -> Completion {
    for category in categories {
        if let Availability::Partial(reason) | Availability::Unavailable(reason) = availability
            .categories
            .get(category)
            .copied()
            .unwrap_or(Availability::Unavailable(Reason::UnavailableSemantics))
        {
            completion = completion.combine(Completion::Incomplete(vec![reason]));
        }
    }

    completion
}

fn indexed(target: &SelectedSource, snapshot: &ProjectSnapshot) -> io::Result<bool> {
    if snapshot
        .source(target.identity)
        .is_some_and(|source| snapshot.is_declaration(source.version))
    {
        return Ok(false);
    }

    let selection = &target.effective.settings.selection;
    let policy = &selection.index;
    let include = Matcher::new(policy.include.as_deref().or(selection.include.as_deref()))?;
    let exclude = Matcher::new(policy.exclude.as_deref().or(selection.exclude.as_deref()))?;

    let path = snapshot
        .origin(target.identity)
        .and_then(SourceOrigin::path);

    if let Some(path) = path
        && assets::declaration(
            path,
            target
                .effective
                .settings
                .environment
                .asset_directory
                .as_deref(),
        )?
    {
        return Ok(false);
    }

    if path.is_some_and(|path| {
        target
            .effective
            .settings
            .environment
            .groups
            .iter()
            .any(|group| {
                group
                    .definitions
                    .iter()
                    .any(|definition| definition == path)
            })
    }) {
        return Ok(false);
    }

    Ok(path.is_none_or(|path| {
        include.includes(path) && (!exclude.matches(path) || policy.override_exclusions)
    }))
}

fn deferred(error: &io::Error) -> bool {
    error.get_ref().and_then(|error| error.downcast_ref::<OperationFailure>()).is_some_and(|failure| failure.kind == FailureKind::Operational && matches!(&failure.completion, Completion::Incomplete(reasons) if reasons.contains(&Reason::UnavailableEnvironment)))
}

fn failed(
    error: Option<io::Error>,
    mut failures: Vec<OperationFailure>,
    completion: Completion,
    diagnostics: Vec<Diagnostic>,
    outcomes: Vec<SourceOutcome>,
    options: &Options,
) -> io::Error {
    let mut failure = match error {
        Some(error) => failure(&error, options, &Mutex::default()),
        None => failures.remove(0),
    };

    failure.completion = failure.completion.combine(completion);

    for diagnostic in diagnostics {
        if !failure.diagnostics.contains(&diagnostic) {
            failure.diagnostics.push(diagnostic);
        }
    }

    for outcome in outcomes {
        if !failure.completed_sources.contains(&outcome) {
            failure.completed_sources.push(outcome);
        }
    }

    for pending in failures {
        if failure.message != pending.message {
            failure.message.push_str("; ");
            failure.message.push_str(&pending.message);
        }

        failure.completion = failure.completion.combine(pending.completion);

        for diagnostic in pending.diagnostics {
            if !failure.diagnostics.contains(&diagnostic) {
                failure.diagnostics.push(diagnostic);
            }
        }

        for source in pending.completed_sources {
            if !failure.completed_sources.contains(&source) {
                failure.completed_sources.push(source);
            }
        }

        for edit in pending.completed_edits {
            if !failure.completed_edits.contains(&edit) {
                failure.completed_edits.push(edit);
            }
        }
    }

    if let Some(interrupted) = options.interrupted() {
        failure.completion = failure.completion.combine(interrupted);
    }

    failure.into()
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use instar_luau::{SemanticAvailability, SemanticCategory};
    use instar_shared::{Availability, Completion, Reason};

    use super::{categories, requested_completion};

    use crate::queries::request::PositionQueryKind;

    #[test]
    fn requested_availability_preserves_missing_and_required_outcomes() {
        let availability = SemanticAvailability {
            categories: BTreeMap::from([
                (SemanticCategory::Bindings, Availability::Available),
                (SemanticCategory::Occurrences, Availability::Available),
                (SemanticCategory::Calls, Availability::Available),
                (
                    SemanticCategory::Types,
                    Availability::Unavailable(Reason::UnavailableSemantics),
                ),
            ]),
        };

        let requested = categories(PositionQueryKind::Signatures);

        assert_eq!(
            requested_completion(Completion::Complete, &availability, &requested),
            Completion::Complete
        );

        for completion in [
            Completion::Incomplete(vec![Reason::UnavailableEnvironment]),
            Completion::Incomplete(vec![Reason::UnavailableSemantics]),
            Completion::Incomplete(vec![Reason::AnalysisLimit]),
            Completion::Incomplete(vec![Reason::FailedPrerequisite]),
            Completion::Cancelled,
            Completion::TimedOut,
        ] {
            assert_eq!(
                requested_completion(completion.clone(), &availability, &requested),
                completion
            );
        }

        let mut missing = availability;
        missing.categories.remove(&SemanticCategory::Calls);

        assert_eq!(
            requested_completion(Completion::Complete, &missing, &requested),
            Completion::Incomplete(vec![Reason::UnavailableSemantics])
        );

        missing.categories.insert(
            SemanticCategory::Calls,
            Availability::Unavailable(Reason::UnavailableEnvironment),
        );

        assert_eq!(
            requested_completion(Completion::Complete, &missing, &requested),
            Completion::Incomplete(vec![Reason::UnavailableEnvironment])
        );
    }
}
