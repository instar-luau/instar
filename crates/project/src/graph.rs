use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::Path,
    sync::Arc,
};

use instar_luau::{
    AnalysisIdentity, AnalysisPurpose, AnalysisRequest, ClosureIdentity, CycleEdge, CyclePath,
    ImportOutcome, ImportRequest, ImportSite, InferenceMode, InferenceSettings, InstanceStep,
    ModuleIdentity, ModuleInput, ReportingSettings, ResolvedImport, SemanticCategory, SourceInput,
};

use instar_shared::{
    Completion, Diagnostic, DiagnosticCategory, FindingIdentity, OperationFailure, Options, Reason,
    Severity, SourceSnapshot,
};

use instar_syntax::{DirectiveKind, SyntaxSettings, directives, parse};

use petgraph::{
    algo::kosaraju_scc,
    graphmap::{DiGraphMap, UnGraphMap},
};

use serde::Serialize;

use crate::{
    capture::Capture,
    configuration::{CheckingMode, EffectiveSettings, OperationOverrides},
    environments::{self, EnvironmentRecipe},
    execution::{Runtime, failure},
    inputs::{InputVersion, SourceOrigin},
    report::{DiagnosticRecord, ImportExplanation, ImportFailure},
    request::{Input, Invocation},
    selection::SelectedSource,
    sourcemap::Map,
};

/// A resolved import between captured module contexts.
#[derive(Clone, Debug, Serialize)]
pub struct DependencyEdge {
    /// Module containing the import.
    pub importer: ModuleIdentity,

    /// Module selected by resolution.
    pub target: ModuleIdentity,

    /// Exact captured call, argument, and import request.
    pub site: ImportSite,
}

pub(crate) struct ModuleNode {
    pub(crate) identity: ModuleIdentity,
    pub(crate) source: SourceSnapshot,
    pub(crate) effective: EffectiveSettings,
    pub(crate) environment: EnvironmentRecipe,
    pub(crate) imports: Vec<ResolvedImport>,
    pub(crate) map: Option<(Arc<Map>, usize)>,
}

pub(crate) struct DependencyGraph {
    pub(crate) modules: BTreeMap<ModuleIdentity, ModuleNode>,
    pub(crate) edges: Vec<DependencyEdge>,
    pub(crate) imports: Vec<ImportExplanation>,
    pub(crate) findings: Vec<DiagnosticRecord>,
    pub(crate) roots: BTreeSet<ModuleIdentity>,
    pub(crate) completion: Completion,
}

pub(crate) struct ClosureRecipe {
    pub(crate) request: AnalysisRequest,
    pub(crate) inputs: Vec<InputVersion>,
}

type ModulePlacement = (ModuleIdentity, Option<(Arc<Map>, usize)>);

struct GraphRequest<'request> {
    invocation: &'request Invocation,
    overrides: &'request OperationOverrides,
    prepare_environment: bool,
}

struct PendingModule {
    identity: ModuleIdentity,
    source: SourceSnapshot,
    effective: EffectiveSettings,
    map: Option<(Arc<Map>, usize)>,
}

struct GraphBuild {
    graph: DependencyGraph,
    pending: Vec<PendingModule>,
}

impl Runtime {
    pub(crate) fn graph(
        &mut self,
        selected: &[SelectedSource],
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        prepare_environment: bool,
        options: &Options,
    ) -> io::Result<DependencyGraph> {
        let request = GraphRequest {
            invocation,
            overrides,
            prepare_environment,
        };

        let mut build = GraphBuild {
            graph: DependencyGraph {
                modules: BTreeMap::new(),
                edges: Vec::new(),
                imports: Vec::new(),
                findings: Vec::new(),
                roots: BTreeSet::new(),
                completion: Completion::Complete,
            },
            pending: Vec::new(),
        };

        self.graph_roots(selected, &request, &mut build, capture, options)?;

        while let Some(module) = build.pending.pop() {
            options.check()?;

            if build.graph.modules.contains_key(&module.identity) {
                continue;
            }

            let prepared = self.graph_node(module, &request, &mut build.graph, capture, options)?;
            self.graph_imports(prepared, &request, &mut build, capture, options)?;
        }

        let mut graph = build.graph;

        if matches!(overrides, OperationOverrides::Lint(_)) {
            self.graph_lint_environment(&mut graph, capture, options)?;
        }

        graph
            .edges
            .sort_by_key(|edge| (edge.importer, edge.target, edge.site.argument.span.start));

        graph.imports.sort_by_key(|import| {
            (
                import.importer,
                import.site.argument.span.start,
                import.site.identity,
            )
        });

        Ok(graph)
    }

    fn graph_roots(
        &self,
        selected: &[SelectedSource],
        request: &GraphRequest<'_>,
        build: &mut GraphBuild,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<()> {
        for selected in selected {
            let source = capture
                .snapshot
                .source(selected.identity)
                .cloned()
                .ok_or_else(|| invalid("selected source is absent"))?;

            let mut contexts =
                self.module_contexts(&source, &selected.effective, capture, options)?;

            let explicit = request
                .invocation
                .inputs
                .iter()
                .filter_map(|input| match input {
                    Input::Module(module) if module.source == selected.identity => Some(*module),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();

            for module in &explicit {
                if contexts.iter().any(|(identity, _)| identity == module) {
                    continue;
                }

                let Some((root, version, placement)) = self.host.module_placement(module.context)
                else {
                    return Err(invalid(
                        "selected module context is obsolete or unavailable",
                    ));
                };

                if root != selected.effective.context {
                    return Err(invalid(
                        "selected module context belongs to a different project context",
                    ));
                }

                let path = capture
                    .snapshot
                    .origin(version.identity)
                    .and_then(SourceOrigin::path)
                    .ok_or_else(|| invalid("selected module sourcemap is unavailable"))?
                    .to_path_buf();

                let source_path = capture
                    .snapshot
                    .origin(source.version.identity)
                    .and_then(SourceOrigin::path)
                    .ok_or_else(|| invalid("selected mapped module has no source path"))?;

                if path
                    .parent()
                    .is_some_and(|directory| source_path.starts_with(directory))
                {
                    return Err(invalid(
                        "selected module context is obsolete or unavailable",
                    ));
                }

                let document = capture.source(&path, options)?;

                if document.version != version {
                    return Err(invalid("selected module sourcemap revision is obsolete"));
                }

                let map = Map::parse(&path, &document)?;

                let source_path = capture
                    .snapshot
                    .origin(source.version.identity)
                    .and_then(SourceOrigin::path)
                    .ok_or_else(|| invalid("selected mapped module has no source path"))?;

                let placements = map.placements(source_path);

                if !placements.is_empty() && !placements.contains(&placement) {
                    return Err(invalid("selected module instance placement is obsolete"));
                }

                contexts.push((*module, Some((map, placement))));
            }

            for (identity, map) in contexts {
                if !explicit.is_empty() && !explicit.contains(&identity) {
                    continue;
                }

                build.graph.roots.insert(identity);

                build.pending.push(PendingModule {
                    identity,
                    source: source.clone(),
                    effective: selected.effective.clone(),
                    map,
                });
            }
        }

        Ok(())
    }

    fn graph_node(
        &mut self,
        module: PendingModule,
        request: &GraphRequest<'_>,
        graph: &mut DependencyGraph,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<(ModuleNode, Vec<ImportSite>)> {
        let PendingModule {
            identity,
            source,
            effective,
            map,
        } = module;

        capture.consult_source(&source);
        let syntax = self.source_syntax(&source, &effective, options)?;
        graph.completion = graph.completion.clone().combine(syntax.completion.clone());

        graph
            .findings
            .extend(
                syntax
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| DiagnosticRecord {
                        module: Some(identity),
                        diagnostic,
                    }),
            );

        let sites = syntax
            .imports
            .as_ref()
            .map_err(|error| {
                let mut failure = failure(error, options, &self.events);
                failure.completion = failure.completion.combine(graph.completion.clone());

                for finding in &graph.findings {
                    if !failure.diagnostics.contains(&finding.diagnostic) {
                        failure.diagnostics.push(finding.diagnostic.clone());
                    }
                }

                io::Error::from(failure)
            })?
            .clone();

        let member_precedence = sites.iter().any(|site| matches!(&site.request, ImportRequest::Instance(path) if path.steps.iter().any(|step| matches!(step, InstanceStep::Field(_)))));

        let requested = if matches!(request.overrides, OperationOverrides::Lint(_)) {
            effective.settings.linting.enabled().any(|(rule, _)| {
                rule.prerequisites.environment || !rule.prerequisites.semantics.is_empty()
            })
        } else {
            request.prepare_environment
        };

        let environment = self.environment(
            &effective,
            map.as_ref(),
            capture,
            requested,
            member_precedence,
            options,
        )?;

        graph.completion = graph
            .completion
            .clone()
            .combine(environment.completion.clone());

        graph
            .findings
            .extend(
                environment
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| DiagnosticRecord {
                        module: Some(identity),
                        diagnostic,
                    }),
            );

        let node = ModuleNode {
            identity,
            source: source.clone(),
            effective,
            environment,
            imports: Vec::new(),
            map,
        };

        Ok((node, sites))
    }

    fn graph_target(
        &mut self,
        node: &ModuleNode,
        site: &ImportSite,
        request: &GraphRequest<'_>,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<Option<PendingModule>> {
        let Some(target) = Self::resolve(node, site, capture, options)? else {
            return Ok(None);
        };

        let target_source = capture.source(&target.path, options)?;
        let mut dependency_overrides = request.overrides.clone();

        if let OperationOverrides::Check(overrides) = &mut dependency_overrides {
            overrides.checking.mode = None;
        }

        let effective = self.effective(
            target_source.version.identity,
            request.invocation,
            &dependency_overrides,
            capture,
            options,
        )?;

        self.prepare_source(&target_source, &effective, options)?;

        let (target_identity, map) = if let Some((map, placement)) = target.map {
            let context = self
                .host
                .module_context(effective.context, map.version, placement)?;

            (
                ModuleIdentity {
                    context,
                    source: target_source.version.identity,
                },
                Some((map, placement)),
            )
        } else {
            let mut placements =
                self.module_contexts(&target_source, &effective, capture, options)?;

            if placements.len() != 1 {
                return Err(invalid("dependency maps to multiple instances"));
            }

            placements
                .pop()
                .ok_or_else(|| invalid("dependency has no module context"))?
        };

        Ok(Some(PendingModule {
            identity: target_identity,
            source: target_source,
            effective,
            map,
        }))
    }

    fn graph_imports(
        &mut self,
        prepared: (ModuleNode, Vec<ImportSite>),
        request: &GraphRequest<'_>,
        build: &mut GraphBuild,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<()> {
        let (mut node, sites) = prepared;

        for site in sites {
            options.check()?;
            capture.begin_consultation();
            let target = self.graph_target(&node, &site, request, capture, options);
            let inputs = capture.end_consultation();

            let (outcome, failure) = match target {
                Ok(Some(target)) => {
                    build.graph.edges.push(DependencyEdge {
                        importer: node.identity,
                        target: target.identity,
                        site: site.clone(),
                    });

                    let identity = target.identity;
                    build.pending.push(target);

                    (ImportOutcome::Module(identity), None)
                }

                Ok(None) => (ImportOutcome::Nonconstant, None),
                Err(error) => resolution_failure(&node, &site, error, &mut build.graph, options)?,
            };

            build.graph.imports.push(ImportExplanation {
                importer: node.identity,
                site: site.clone(),
                outcome: outcome.clone(),
                failure,
                inputs,
            });

            node.imports.push(ResolvedImport { site, outcome });
        }

        build.graph.modules.insert(node.identity, node);

        Ok(())
    }

    fn graph_lint_environment(
        &self,
        graph: &mut DependencyGraph,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<()> {
        let mut connected = UnGraphMap::<ModuleIdentity, ()>::new();

        for identity in graph.modules.keys() {
            connected.add_node(*identity);
        }

        for edge in &graph.edges {
            connected.add_edge(edge.importer, edge.target, ());
        }

        for component in kosaraju_scc(&connected) {
            if !component.iter().any(|identity| {
                graph.modules[identity]
                    .effective
                    .settings
                    .linting
                    .enabled()
                    .any(|(rule, _)| !rule.prerequisites.semantics.is_empty())
            }) {
                continue;
            }

            for identity in component {
                let node = graph
                    .modules
                    .get_mut(&identity)
                    .ok_or_else(|| invalid("dependency module is absent"))?;

                if node.environment.requested {
                    continue;
                }

                node.environment = self.environment(
                    &node.effective,
                    node.map.as_ref(),
                    capture,
                    true,
                    false,
                    options,
                )?;

                graph.completion = graph
                    .completion
                    .clone()
                    .combine(node.environment.completion.clone());

                graph
                    .findings
                    .extend(
                        node.environment
                            .diagnostics
                            .iter()
                            .cloned()
                            .map(|diagnostic| DiagnosticRecord {
                                module: Some(identity),
                                diagnostic,
                            }),
                    );
            }
        }

        Ok(())
    }

    pub(crate) fn module_contexts(
        &self,
        source: &SourceSnapshot,
        effective: &EffectiveSettings,
        capture: &mut Capture,
        options: &Options,
    ) -> io::Result<Vec<ModulePlacement>> {
        let Some(path) = capture
            .snapshot
            .origin(source.version.identity)
            .and_then(SourceOrigin::path)
            .map(Path::to_path_buf)
        else {
            return Ok(vec![(
                ModuleIdentity {
                    context: effective.context,
                    source: source.version.identity,
                },
                None,
            )]);
        };

        let mut contexts = Vec::new();

        for map_path in &effective.settings.resolution.sourcemaps {
            let map_source = capture.source(map_path, options)?;
            let map = Map::parse(map_path, &map_source)?;

            for node in map.placements(&path) {
                let context = self
                    .host
                    .module_context(effective.context, map.version, node)?;

                contexts.push((
                    ModuleIdentity {
                        context,
                        source: source.version.identity,
                    },
                    Some((Arc::clone(&map), node)),
                ));
            }
        }

        if contexts.is_empty() {
            contexts.push((
                ModuleIdentity {
                    context: effective.context,
                    source: source.version.identity,
                },
                None,
            ));
        }

        Ok(contexts)
    }

    pub(crate) fn closures(
        &mut self,
        graph: &DependencyGraph,
        purpose: AnalysisPurpose,
        semantic: &[SemanticCategory],
        capture: &Capture,
        options: &Options,
    ) -> io::Result<(Vec<ClosureRecipe>, Completion, Vec<DiagnosticRecord>)> {
        let mut connected = UnGraphMap::<ModuleIdentity, ()>::new();
        let mut directed = DiGraphMap::<ModuleIdentity, ()>::new();

        for identity in graph.modules.keys() {
            connected.add_node(*identity);
            directed.add_node(*identity);
        }

        for edge in &graph.edges {
            connected.add_edge(edge.importer, edge.target, ());
            directed.add_edge(edge.importer, edge.target, ());
        }

        let components = kosaraju_scc(&connected);
        let cyclic = kosaraju_scc(&directed);
        let mut recipes = Vec::new();
        let mut completion = graph.completion.clone();
        let mut diagnostics = Vec::new();

        for mut component in components {
            options.check()?;
            component.sort();
            let mut profile = None;
            let mut compatible = true;

            for identity in &component {
                let node = &graph.modules[identity];

                let current = self.profile(&node.effective.settings.language.flags)?;

                if profile.as_ref().is_some_and(|profile| profile != &current) {
                    compatible = false;
                } else if profile.is_none() {
                    profile = Some(current);
                }
            }

            if !compatible {
                completion =
                    completion.combine(Completion::Incomplete(vec![Reason::UnsupportedFeature]));

                for identity in &component {
                    diagnostics.push(DiagnosticRecord {
                        module: Some(*identity),
                        diagnostic: environments::diagnostic(&graph.modules[identity].source, "analysis.cross_profile", DiagnosticCategory::Analysis, "imports span incompatible process-global language profiles; cross-profile semantic exports are unavailable".to_owned()),
                    });
                }

                continue;
            }

            let revision = self.identity();

            let modules = component
                .iter()
                .map(|identity| {
                    closure_module(
                        *identity,
                        &graph.modules[identity],
                        revision,
                        purpose,
                        semantic,
                        options,
                    )
                })
                .collect::<Vec<_>>();

            let mut cycles = Vec::new();

            for component in &cyclic {
                if component.len() > 1
                    || component
                        .first()
                        .is_some_and(|identity| directed.contains_edge(*identity, *identity))
                {
                    let members = component.iter().copied().collect::<BTreeSet<_>>();

                    if members.iter().all(|identity| {
                        modules
                            .iter()
                            .any(|module| module.analysis.module == *identity)
                    }) {
                        cycles.extend(cycle_paths(&members, &graph.edges, options)?);
                    }
                }
            }

            recipes.push(ClosureRecipe {
                request: AnalysisRequest {
                    closure: ClosureIdentity::new(revision),
                    profile: profile.ok_or_else(|| invalid("empty captured closure"))?,
                    purpose,
                    roots: graph
                        .roots
                        .intersection(&component.into_iter().collect())
                        .copied()
                        .collect(),
                    modules,
                    cycles,
                },
                inputs: capture.guards(),
            });
        }

        recipes.sort_by_key(|recipe| recipe.request.roots.first().copied());

        Ok((recipes, completion, diagnostics))
    }
}

pub(crate) fn syntax_settings(effective: &EffectiveSettings) -> SyntaxSettings {
    effective.settings.syntax.clone()
}

fn resolution_failure(
    node: &ModuleNode,
    site: &ImportSite,
    error: io::Error,
    graph: &mut DependencyGraph,
    options: &Options,
) -> io::Result<(ImportOutcome, Option<ImportFailure>)> {
    options.check()?;

    if matches!(
        error.kind(),
        io::ErrorKind::Interrupted | io::ErrorKind::TimedOut
    ) {
        return Err(error);
    }

    let primary = site.argument;

    if let Some(failure) = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<OperationFailure>())
    {
        graph.completion = graph.completion.clone().combine(failure.completion.clone());

        graph
            .findings
            .extend(
                failure
                    .diagnostics
                    .iter()
                    .cloned()
                    .map(|diagnostic| DiagnosticRecord {
                        module: Some(node.identity),
                        diagnostic,
                    }),
            );
    } else {
        let diagnostic = Diagnostic {
            identity: FindingIdentity::new(
                "project",
                vec![node.source.version],
                "resolution.unresolved",
                primary,
            ),
            code: "resolution.unresolved".to_owned(),
            category: DiagnosticCategory::Resolution,
            severity: Severity::Error,
            message: error.to_string(),
            primary,
            related: Vec::new(),
            tags: Vec::new(),
        };

        graph.findings.push(DiagnosticRecord {
            module: Some(node.identity),
            diagnostic,
        });
    }

    if error.kind() == io::ErrorKind::Unsupported {
        graph.completion = graph
            .completion
            .clone()
            .combine(Completion::Incomplete(vec![Reason::UnavailableSemantics]));
    }

    Ok((
        ImportOutcome::Unresolved,
        Some(ImportFailure {
            kind: error.kind(),
            message: error.to_string(),
        }),
    ))
}

fn closure_module(
    identity: ModuleIdentity,
    node: &ModuleNode,
    revision: u64,
    purpose: AnalysisPurpose,
    semantic: &[SemanticCategory],
    options: &Options,
) -> ModuleInput {
    let mode = source_mode(node, options);

    let required =
        purpose == AnalysisPurpose::Editor || !semantic.is_empty() || mode != CheckingMode::Nocheck;

    let available = required
        && node.environment.requested
        && node.environment.completion == Completion::Complete;

    ModuleInput {
        analysis: AnalysisIdentity {
            module: identity,
            revision,
        },
        source: SourceInput {
            module: identity,
            source: node.source.clone(),
            language: node.effective.settings.language.clone(),
        },
        environment: available.then(|| node.environment.input.clone()),
        inference: InferenceSettings {
            required: available,
            mode: if mode == CheckingMode::Strict {
                InferenceMode::Strict
            } else {
                InferenceMode::Nonstrict
            },
            retained: if available {
                semantic.to_vec()
            } else {
                Vec::new()
            },
        },
        reporting: ReportingSettings {
            ordinary_type_diagnostics: available
                && purpose == AnalysisPurpose::Source
                && semantic.is_empty()
                && mode != CheckingMode::Nocheck
                && node.effective.settings.type_errors,
        },
        imports: node.imports.clone(),
    }
}

pub(crate) fn source_mode(node: &ModuleNode, options: &Options) -> CheckingMode {
    if node.effective.settings.checking_override {
        return node.effective.settings.checking;
    }

    let parsed = parse(&node.source, &syntax_settings(&node.effective), options);

    for directive in directives(&parsed.tree, options).directives {
        if directive.scope.start != 0 || directive.scope.end != node.source.text.len() {
            continue;
        }

        if let DirectiveKind::Source(mode) = directive.kind {
            match mode.as_str() {
                "!strict" => return CheckingMode::Strict,
                "!nonstrict" => return CheckingMode::Nonstrict,
                "!nocheck" => return CheckingMode::Nocheck,
                _ => {}
            }
        }
    }

    node.effective.settings.checking
}

fn cycle_paths(
    members: &BTreeSet<ModuleIdentity>,
    edges: &[DependencyEdge],
    options: &Options,
) -> io::Result<Vec<CyclePath>> {
    let mut paths = Vec::new();

    for start in members {
        let mut pending = vec![(*start, Vec::<CycleEdge>::new(), BTreeSet::new())];

        while let Some((current, path, mut visited)) = pending.pop() {
            options.check()?;

            if !visited.insert(current) {
                continue;
            }

            for edge in edges
                .iter()
                .filter(|edge| edge.importer == current && members.contains(&edge.target))
            {
                let mut path = path.clone();

                path.push(CycleEdge {
                    from: edge.importer,
                    to: edge.target,
                    site: edge.site.call,
                });

                if edge.target == *start {
                    paths.push(CyclePath { edges: path });
                    pending.clear();
                    break;
                }

                if !visited.contains(&edge.target) {
                    pending.push((edge.target, path, visited.clone()));
                }
            }
        }
    }

    Ok(paths)
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
