//! Positive and negative fixtures for every configured Instar rule.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use instar_lint::{
    DeprecatedSymbols, LintSettings, Preset, ResolvedDependency, RestrictedGlobals,
    RestrictedImports, RuleIdentity, RuleInput, RuleProducer, RuleResult, RuleSettings,
};

use instar_luau::{
    AnalysisIdentity, BindingFact, BindingKind, CallFact, CallableSignature, ControlFlow,
    Determinism, EffectFact, EnvironmentIdentity, ExpressionFact, FactsResult, ModuleIdentity,
    SemanticAvailability, SemanticCategory, SemanticFacts, SemanticIdentity, SemanticKind,
    SignatureParameter, TypeFact,
};

use instar_shared::{
    Availability, Completion, ContextIdentity, DiagnosticCategory, EventPayload, Knowledge,
    Location, Options, Reason, Severity, SourceIdentity, SourceSnapshot, SourceVersion, Span,
};

use instar_syntax::{SyntaxSettings, bindings, directives, parse, tree::NodeKind};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fixture {
    rule: String,
    range: [usize; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSettings {
    deprecated: DeprecatedSymbols,
    restricted_globals: RestrictedGlobals,
    restricted_module_paths: RestrictedImports,
}

fn fixtures() -> BTreeMap<String, Fixture> {
    toml::from_str(include_str!("fixtures/inventory.toml")).expect("fixture inventory")
}

fn configured(rule: RuleIdentity) -> LintSettings {
    let fixtures: FixtureSettings =
        toml::from_str(include_str!("fixtures/configuration.toml")).expect("fixture configuration");

    let mut settings = LintSettings::default();
    settings.apply_preset(Preset::None);
    settings.roblox = true;
    let configuration = settings.rules.get_mut(&rule).expect("catalog rule");
    configuration.enabled = true;

    configuration.settings = match rule {
        RuleIdentity::Deprecated => RuleSettings::Deprecated(fixtures.deprecated),

        RuleIdentity::RestrictedGlobals => {
            RuleSettings::RestrictedGlobals(fixtures.restricted_globals)
        }

        RuleIdentity::RestrictedModulePaths => {
            RuleSettings::RestrictedModulePaths(fixtures.restricted_module_paths)
        }

        _ => RuleSettings::for_rule(rule),
    };

    settings
}

fn snapshot(text: &str) -> io::Result<SourceSnapshot> {
    SourceSnapshot::new(
        SourceVersion {
            identity: SourceIdentity::new(1),
            revision: 7,
        },
        text,
    )
}

fn options() -> Options {
    Options::new(Some(Instant::now() + Duration::from_secs(5)))
}

fn location(range: [usize; 2]) -> Location {
    Location {
        source: SourceVersion {
            identity: SourceIdentity::new(1),
            revision: 7,
        },
        span: Span {
            start: range[0],
            end: range[1],
        },
    }
}

fn dependency(
    call: [usize; 2],
    argument: [usize; 2],
    constant: bool,
    path: Option<&str>,
) -> ResolvedDependency {
    ResolvedDependency {
        call: location(call),
        argument: location(argument),
        constant,
        requested: Knowledge::Known(path.map(str::to_owned)),
        target: Knowledge::Known(None),
        availability: Availability::Available,
    }
}

fn facts() -> FactsResult {
    FactsResult {
        analysis: AnalysisIdentity {
            module: ModuleIdentity {
                context: ContextIdentity::new(1),
                source: SourceIdentity::new(1),
            },
            revision: 7,
        },
        facts: SemanticFacts::default(),
        availability: SemanticAvailability {
            categories: [
                SemanticCategory::Bindings,
                SemanticCategory::Types,
                SemanticCategory::Constants,
                SemanticCategory::Calls,
                SemanticCategory::Effects,
                SemanticCategory::Dependencies,
                SemanticCategory::Occurrences,
            ]
            .into_iter()
            .map(|category| (category, Availability::Available))
            .collect(),
        },
        diagnostics: Vec::new(),
        completion: Completion::Complete,
    }
}

fn binding_fact(range: [usize; 2], parameter: bool, is_any: bool) -> BindingFact {
    let declaration = location(range);

    BindingFact {
        identity: SemanticIdentity::Source {
            location: declaration,
            kind: if parameter {
                SemanticKind::Parameter
            } else {
                SemanticKind::Binding
            },
            ordinal: 0,
        },
        declaration: Knowledge::Known(Some(declaration)),
        name: "value".to_owned(),
        kind: if parameter {
            BindingKind::Parameter
        } else {
            BindingKind::Local
        },
        annotation: false,
        scope: SemanticIdentity::Source {
            location: location([0, 0]),
            kind: SemanticKind::Scope,
            ordinal: 0,
        },
        shadows: Knowledge::Known(None),
        type_information: Knowledge::Known(TypeFact {
            display: if is_any { "any" } else { "number" }.to_owned(),
            is_any,
            declarations: Vec::new(),
        }),
        documentation: None,
        deprecated: false,
        language_identity: None,
    }
}

fn environment_identity(name: String, kind: SemanticKind) -> SemanticIdentity {
    SemanticIdentity::Environment {
        environment: EnvironmentIdentity::new(1),
        namespace: "fixture".to_owned(),
        name,
        kind,
    }
}

fn invariant_facts(text: &str) -> io::Result<FactsResult> {
    let source = snapshot(text)?;
    let syntax = parse(&source, &SyntaxSettings::default(), &options());
    let tree = syntax.tree.syntax();
    let mut output = facts();

    for node in &tree.nodes {
        let span = [node.span.start, node.span.end];
        let location = location(span);

        let expression = matches!(
            node.kind,
            NodeKind::Name { .. }
                | NodeKind::Field { .. }
                | NodeKind::Index { .. }
                | NodeKind::Call { .. }
                | NodeKind::MethodCall { .. }
                | NodeKind::Binary { .. }
                | NodeKind::Unary { .. }
                | NodeKind::Table { .. }
                | NodeKind::Number { .. }
        );

        if !expression {
            continue;
        }

        let name = &text[span[0]..span[1]];

        let call = matches!(
            node.kind,
            NodeKind::Call { .. } | NodeKind::MethodCall { .. }
        );

        let pure = !call || name == "math.abs(1)";

        let dependencies = if name.starts_with("math") {
            vec![environment_identity(
                "math".to_owned(),
                SemanticKind::Binding,
            )]
        } else if matches!(node.kind, NodeKind::Name { .. }) {
            vec![environment_identity(name.to_owned(), SemanticKind::Binding)]
        } else {
            Vec::new()
        };

        let effect = EffectFact {
            reads: dependencies.clone(),
            writes: Vec::new(),
            may_throw: !pure,
            may_yield: !pure,
            may_allocate: !pure,
            determinism: if pure {
                Determinism::Deterministic
            } else {
                Determinism::Unproven
            },
            control_flow: if pure {
                ControlFlow::Preserved
            } else {
                ControlFlow::Unproven
            },
        };

        output.facts.expressions.push(ExpressionFact {
            location,
            type_information: Knowledge::Unavailable(Reason::UnavailableSemantics),
            constant: Knowledge::Known(None),
            dependencies: Knowledge::Known(dependencies),
            effects: Knowledge::Known(effect.clone()),
        });

        if call {
            output.facts.calls.push(CallFact {
                location,
                caller: environment_identity("fixture".to_owned(), SemanticKind::Function),
                targets: Knowledge::Known(vec![environment_identity(
                    name.to_owned(),
                    SemanticKind::Function,
                )]),
                signatures: Knowledge::Known(Vec::new()),
                arguments: Vec::new(),
                effects: Knowledge::Known(effect),
            });
        }
    }

    Ok(output)
}

fn argument_facts(text: &str) -> io::Result<FactsResult> {
    let source = snapshot(text)?;
    let syntax = parse(&source, &SyntaxSettings::default(), &options());
    let lexical = bindings(&syntax.tree, &options());
    let tree = syntax.tree.syntax();
    let mut output = facts();
    let mut pending = vec![tree.root];

    while let Some(node) = pending.pop() {
        if let NodeKind::Call { callee, .. } = &tree.node(node).kind {
            let span = tree.node(node).span;

            let signatures =
                lexical
                    .arities
                    .get(&callee.get())
                    .map_or_default(|(count, variadic)| {
                        vec![CallableSignature {
                            label: "fixture".to_owned(),
                            parameters: (0..*count)
                                .map(|_| SignatureParameter {
                                    label_range: [0, 0],
                                    declaration: None,
                                })
                                .collect(),
                            variadic: *variadic,
                            minimum_arguments: Knowledge::Known(*count),
                            documentation: None,
                        }]
                    });

            output.facts.calls.push(CallFact {
                location: location([span.start, span.end]),
                caller: environment_identity("fixture".to_owned(), SemanticKind::Function),
                targets: Knowledge::Known(vec![environment_identity(
                    String::from_utf8_lossy(tree.text(*callee)).into_owned(),
                    SemanticKind::Function,
                )]),
                signatures: Knowledge::Known(signatures),
                arguments: Vec::new(),
                effects: Knowledge::Unavailable(Reason::UnavailableSemantics),
            });
        }

        pending.extend(tree.children(node));
    }

    Ok(output)
}

fn run(
    text: &str,
    settings: &LintSettings,
    imports: &[ResolvedDependency],
    facts: Option<&FactsResult>,
) -> io::Result<RuleResult> {
    run_with_options(text, settings, imports, facts, &options())
}

fn run_with_options(
    text: &str,
    settings: &LintSettings,
    imports: &[ResolvedDependency],
    facts: Option<&FactsResult>,
    options: &Options,
) -> io::Result<RuleResult> {
    let source = snapshot(text)?;
    let syntax = parse(&source, &SyntaxSettings::default(), options);
    let bindings = bindings(&syntax.tree, options);
    let directives = directives(&syntax.tree, options);

    let arguments = if facts.is_none()
        && settings
            .rules
            .get(&RuleIdentity::MismatchedArgumentCount)
            .is_some_and(|configuration| configuration.enabled)
    {
        Some(argument_facts(text)?)
    } else {
        None
    };

    let facts = facts.or(arguments.as_ref());

    Ok(instar_lint::run(
        &RuleInput {
            source: &source,
            syntax: &syntax.tree,
            bindings: &bindings,
            facts,
            imports,
            settings,
            directives: &directives,
        },
        options,
    ))
}

fn assert_findings(result: &RuleResult, rule: RuleIdentity, ranges: &[[usize; 2]]) {
    assert_eq!(result.completion, Completion::Complete, "{rule}");

    assert!(
        result
            .availability
            .iter()
            .all(|availability| availability.rule == rule
                && availability.status == Availability::Available)
    );

    assert_eq!(
        result.findings.len(),
        ranges.len(),
        "{rule}: {:?}",
        result.findings
    );

    for (diagnostic, range) in result.findings.iter().zip(ranges) {
        assert_eq!(diagnostic.category, DiagnosticCategory::Lint);
        assert_eq!(diagnostic.code, rule.name());
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.primary.source.identity, SourceIdentity::new(1));
        assert_eq!(diagnostic.primary.source.revision, 7);

        assert_eq!(
            [diagnostic.primary.span.start, diagnostic.primary.span.end],
            *range
        );

        assert_eq!(diagnostic.related, Vec::new());
    }
}

#[test]
fn every_syntax_rule_has_positive_and_negative_fixtures() -> io::Result<()> {
    for (name, fixture) in fixtures() {
        let rule = RuleIdentity::named(&fixture.rule).expect("known fixture rule");

        let configuration = configured(rule);

        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);

        let positive = fs::read_to_string(directory.join("positive.luau"))?;
        let negative = fs::read_to_string(directory.join("negative.luau"))?;

        let positive_facts = (rule == RuleIdentity::LoopInvariantCall)
            .then(|| invariant_facts(&positive))
            .transpose()?;

        let negative_facts = (rule == RuleIdentity::LoopInvariantCall)
            .then(|| invariant_facts(&negative))
            .transpose()?;

        let result = run(&positive, &configuration, &[], positive_facts.as_ref())?;
        assert_findings(&result, rule, &[fixture.range]);

        assert_findings(
            &run(&negative, &configuration, &[], negative_facts.as_ref())?,
            rule,
            &[],
        );

        assert!(result.findings[0].primary.span.end <= positive.len());
    }

    Ok(())
}

#[test]
fn empty_conditional_branches_highlight_opening_headers() -> io::Result<()> {
    let configuration = configured(RuleIdentity::EmptyIf);

    for contents in ["", "-- pending", "--[[pending]]"] {
        let text = format!(
            "if flag then\n{contents}\nelseif other then\n{contents}\nelse\n{contents}\nend"
        );

        let ranges = ["if flag then", "elseif other then", "else"].map(|header| {
            let start = text.rfind(header).expect("branch header");

            [start, start + header.len()]
        });

        let result = run(&text, &configuration, &[], None)?;
        assert_findings(&result, RuleIdentity::EmptyIf, &ranges);

        for diagnostic in &result.findings {
            assert_eq!(diagnostic.message, "empty conditional branch");
        }
    }

    let text = "if flag then return 1 elseif other then else return 2 end";
    let header = "elseif other then";
    let start = text.find(header).expect("branch header");

    assert_findings(
        &run(text, &configuration, &[], None)?,
        RuleIdentity::EmptyIf,
        &[[start, start + header.len()]],
    );

    Ok(())
}

#[test]
fn empty_loops_highlight_opening_headers() -> io::Result<()> {
    let configuration = configured(RuleIdentity::EmptyLoop);

    for (header, closing) in [
        ("while flag do", "end"),
        ("repeat", "until flag"),
        ("for index = 1, 4, 2 do", "end"),
        ("for key, value in pairs(values) do", "end"),
    ] {
        for contents in ["", "-- pending", "--[[pending]]"] {
            let text = format!("{header}\n{contents}\n{closing}");
            let result = run(&text, &configuration, &[], None)?;
            assert_findings(&result, RuleIdentity::EmptyLoop, &[[0, header.len()]]);
            assert_eq!(result.findings[0].message, "empty loop body");
        }

        assert_findings(
            &run(
                &format!("{header}\n-- pending\nbreak\n{closing}"),
                &configuration,
                &[],
                None,
            )?,
            RuleIdentity::EmptyLoop,
            &[],
        );
    }

    Ok(())
}

#[test]
fn typed_require_fixtures() -> io::Result<()> {
    let configuration = configured(RuleIdentity::NonConstRequire);

    let dynamic = dependency([0, 13], [8, 12], false, None);

    let constant = dependency([0, 18], [8, 17], true, Some("./clear"));

    assert_findings(
        &run(
            include_str!("fixtures/requires/dynamic.luau"),
            &configuration,
            &[dynamic],
            None,
        )?,
        RuleIdentity::NonConstRequire,
        &[[8, 12]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[constant],
            None,
        )?,
        RuleIdentity::NonConstRequire,
        &[],
    );

    let configuration = configured(RuleIdentity::RestrictedModulePaths);

    let blocked = dependency([0, 20], [8, 19], true, Some("./blocked"));

    let clear = dependency([0, 18], [8, 17], true, Some("./clear"));

    assert_findings(
        &run(
            include_str!("fixtures/requires/blocked.luau"),
            &configuration,
            &[blocked],
            None,
        )?,
        RuleIdentity::RestrictedModulePaths,
        &[[8, 19]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[clear],
            None,
        )?,
        RuleIdentity::RestrictedModulePaths,
        &[],
    );

    Ok(())
}

#[test]
fn native_inference_fixtures_and_unavailable_capability() -> io::Result<()> {
    for (rule, text, parameter) in [
        (
            RuleIdentity::ImplicitAnyLocal,
            include_str!("fixtures/inference/local.luau"),
            false,
        ),
        (
            RuleIdentity::ImplicitAnyParameter,
            include_str!("fixtures/inference/parameter.luau"),
            true,
        ),
    ] {
        let settings = configured(rule);
        let start = text.find("value").expect("binding name");
        let mut supplied = facts();

        supplied
            .facts
            .bindings
            .push(binding_fact([start, start + 5], parameter, true));

        assert_findings(
            &run(text, &settings, &[], Some(&supplied))?,
            rule,
            &[[start, start + 5]],
        );

        supplied.facts.bindings[0].type_information = Knowledge::Known(TypeFact {
            display: "number".to_owned(),
            is_any: false,
            declarations: Vec::new(),
        });

        assert_findings(&run(text, &settings, &[], Some(&supplied))?, rule, &[]);
        let unsupported = run(text, &settings, &[], None)?;

        assert_eq!(
            unsupported.completion,
            Completion::Incomplete(vec![Reason::UnavailableSemantics])
        );

        assert_eq!(unsupported.availability.len(), 1);
        assert_eq!(unsupported.availability[0].rule, rule);

        assert_eq!(
            unsupported.availability[0].status,
            Availability::Unavailable(Reason::UnavailableSemantics)
        );

        assert_eq!(unsupported.findings.len(), 1);
        let diagnostic = &unsupported.findings[0];
        assert_eq!(diagnostic.category, DiagnosticCategory::Analysis);
        assert_eq!(diagnostic.code, "lint.prerequisite");
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.primary.source.identity, SourceIdentity::new(1));
        assert_eq!(diagnostic.primary.source.revision, 7);
        assert_eq!(diagnostic.primary.span, Span { start: 0, end: 0 });
        assert_eq!(diagnostic.related, Vec::new());
    }

    Ok(())
}

#[test]
fn fixture_inventory_matches_configuration() {
    let fixtures = fixtures();

    let covered = fixtures
        .values()
        .map(|fixture| fixture.rule.as_str())
        .chain(
            [
                RuleIdentity::NonConstRequire,
                RuleIdentity::RestrictedModulePaths,
                RuleIdentity::ImplicitAnyLocal,
                RuleIdentity::ImplicitAnyParameter,
            ]
            .map(RuleIdentity::name),
        )
        .collect::<BTreeSet<_>>();

    let configured = instar_lint::rules()
        .iter()
        .filter(|definition| matches!(definition.producer, RuleProducer::Instar(_)))
        .map(|definition| definition.identity.name())
        .collect::<BTreeSet<_>>();

    assert_eq!(covered, configured);
    assert_eq!(configured.len(), 40);
    assert_eq!(instar_lint::rules().len(), 69);
}

#[test]
fn rule_severity_enablement_and_cooperative_limits() -> io::Result<()> {
    let mut settings = configured(RuleIdentity::DivideByZero);

    for severity in [Severity::Information, Severity::Warning, Severity::Error] {
        settings
            .rules
            .get_mut(&RuleIdentity::DivideByZero)
            .expect("rule")
            .severity = severity;

        assert_eq!(
            run("return 1/0", &settings, &[], None)?.findings[0].severity,
            severity
        );
    }

    settings
        .rules
        .get_mut(&RuleIdentity::DivideByZero)
        .expect("rule")
        .enabled = false;

    assert_findings(
        &run("return 1/0", &settings, &[], None)?,
        RuleIdentity::DivideByZero,
        &[],
    );

    let options = Options::new(Some(Instant::now()));

    assert_eq!(
        run_with_options("return 1", &settings, &[], None, &options)?.completion,
        Completion::TimedOut
    );

    let options = Options::new(Some(Instant::now() + Duration::from_secs(5)));
    options.cancellation.cancel();

    assert_eq!(
        run_with_options("return 1", &settings, &[], None, &options)?.completion,
        Completion::Cancelled
    );

    Ok(())
}

#[test]
fn lexical_binding_options_and_shadowing() -> io::Result<()> {
    let mut configuration = configured(RuleIdentity::UnusedVariable);

    let RuleSettings::UnusedVariable(settings) = &mut configuration
        .rules
        .get_mut(&RuleIdentity::UnusedVariable)
        .expect("rule")
        .settings
    else {
        panic!("unused settings")
    };

    settings.parameters = true;
    settings.loop_variables = true;
    settings.ignore_pattern = "^skip".to_owned();

    let result = run(
        include_str!("fixtures/shadowing/input.luau"),
        &configuration,
        &[],
        None,
    )?;

    let source = include_str!("fixtures/shadowing/input.luau");
    let parameter = source.find("value").expect("parameter");
    let binding = source.find("kept").expect("loop binding");

    assert_findings(
        &result,
        RuleIdentity::UnusedVariable,
        &[[parameter, parameter + 5], [binding, binding + 4]],
    );

    let mut configuration = configured(RuleIdentity::PreferConst);

    let RuleSettings::PreferConst(settings) = &mut configuration
        .rules
        .get_mut(&RuleIdentity::PreferConst)
        .expect("rule")
        .settings
    else {
        panic!("immutable settings")
    };

    settings.mutated_tables_stay_local = true;

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/mutation.luau"),
            &configuration,
            &[],
            None,
        )?,
        RuleIdentity::PreferConst,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/table.luau"),
            &configuration,
            &[],
            None,
        )?,
        RuleIdentity::PreferConst,
        &[[6, 7]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/function.luau"),
            &configured(RuleIdentity::MismatchedArgumentCount),
            &[],
            None,
        )?,
        RuleIdentity::MismatchedArgumentCount,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/protection.luau"),
            &configured(RuleIdentity::IgnoredPcallResult),
            &[],
            None,
        )?,
        RuleIdentity::IgnoredPcallResult,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/color.luau"),
            &configured(RuleIdentity::RobloxIncorrectColor3NewBounds),
            &[],
            None,
        )?,
        RuleIdentity::RobloxIncorrectColor3NewBounds,
        &[],
    );

    Ok(())
}

#[test]
fn syntax_errors_are_explicit() -> io::Result<()> {
    let text = include_str!("fixtures/syntax/input.luau");
    let result = run(text, &configured(RuleIdentity::DivideByZero), &[], None)?;
    let source = snapshot(text)?;
    let tree = parse(&source, &SyntaxSettings::default(), &options());

    assert_eq!(
        result.completion,
        Completion::Incomplete(vec![Reason::UnsupportedSyntax])
    );

    assert_eq!(result.availability.len(), 1);
    assert_ne!(result.findings, Vec::new());

    let syntax = result
        .findings
        .iter()
        .filter(|finding| finding.category == DiagnosticCategory::Syntax)
        .collect::<Vec<_>>();

    assert_eq!(syntax.len(), tree.diagnostics.len());

    for (diagnostic, expected) in syntax.iter().zip(&tree.diagnostics) {
        assert_eq!(diagnostic.category, DiagnosticCategory::Syntax);
        assert_eq!(diagnostic.severity, Severity::Error);
        assert_eq!(diagnostic.primary.source.identity, SourceIdentity::new(1));
        assert_eq!(diagnostic.primary.source.revision, 7);
        assert_eq!(diagnostic.primary.span, expected.primary.span);
        assert_eq!(diagnostic.message, expected.message);
        assert_eq!(diagnostic.related, Vec::new());
    }

    Ok(())
}

#[test]
fn service_rules_use_literal_values_and_byte_escapes_remain_valid() -> io::Result<()> {
    let configuration = configured(RuleIdentity::RobloxPreferGetPlayers);

    for text in [
        include_str!("fixtures/services/long.luau"),
        include_str!("fixtures/services/escaped.luau"),
    ] {
        assert_findings(
            &run(text, &configuration, &[], None)?,
            RuleIdentity::RobloxPreferGetPlayers,
            &[[7, text.len()]],
        );
    }

    assert_findings(
        &run(
            include_str!("fixtures/services/other.luau"),
            &configuration,
            &[],
            None,
        )?,
        RuleIdentity::RobloxPreferGetPlayers,
        &[],
    );

    let configuration = configured(RuleIdentity::BadStringEscape);

    assert_findings(
        &run(
            include_str!("fixtures/escapes/byte.luau"),
            &configuration,
            &[],
            None,
        )?,
        RuleIdentity::BadStringEscape,
        &[],
    );

    Ok(())
}

#[test]
fn cancellation_during_diagnostic_construction_updates_completion() -> io::Result<()> {
    let mut operation = options();
    let events = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&events);
    let cancellation = operation.cancellation.clone();

    operation.observer = Some(Arc::new(move |event| {
        if let EventPayload::Progress(progress) = event.payload
            && progress.phase == "Collecting lint findings"
            && observed.fetch_add(1, Ordering::Relaxed) + 1 == 2
        {
            cancellation.cancel();
        }
    }));

    let result = run_with_options(
        "return 1/0",
        &configured(RuleIdentity::DivideByZero),
        &[],
        None,
        &operation,
    )?;

    assert!(events.load(Ordering::Relaxed) >= 2);
    assert_eq!(result.completion, Completion::Cancelled);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].code, RuleIdentity::DivideByZero.name());

    Ok(())
}
