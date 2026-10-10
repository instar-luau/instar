//! Lint rule behavior and interruption checks.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use instar_lint::{
    DeprecatedSymbols, LintSettings, Preset, ResolvedDependency, RestrictedGlobals,
    RestrictedImports, RuleIdentity, RuleInput, RuleResult, RuleSettings,
};

use instar_shared::{
    Availability, Completion, DiagnosticCategory, EventPayload, Knowledge, Location, Options,
    Reason, Severity, SourceIdentity, SourceSnapshot, SourceVersion, Span,
};

use instar_syntax::{SyntaxSettings, bindings, directives, parse};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureSettings {
    deprecated: DeprecatedSymbols,
    restricted_globals: RestrictedGlobals,
    restricted_module_paths: RestrictedImports,
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

fn run(
    text: &str,
    settings: &LintSettings,
    imports: &[ResolvedDependency],
) -> io::Result<RuleResult> {
    run_with_options(text, settings, imports, &options())
}

fn run_with_options(
    text: &str,
    settings: &LintSettings,
    imports: &[ResolvedDependency],
    options: &Options,
) -> io::Result<RuleResult> {
    let source = snapshot(text)?;
    let syntax = parse(&source, &SyntaxSettings::default(), options);
    let bindings = bindings(&syntax.tree, options);
    let directives = directives(&syntax.tree, options);

    Ok(instar_lint::run(
        &RuleInput {
            source: &source,
            syntax: &syntax.tree,
            bindings: &bindings,
            facts: None,
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

        let result = run(&text, &configuration, &[])?;
        assert_findings(&result, RuleIdentity::EmptyIf, &ranges);

        for diagnostic in &result.findings {
            assert_eq!(diagnostic.message, "empty conditional branch");
        }
    }

    let text = "if flag then return 1 elseif other then else return 2 end";
    let header = "elseif other then";
    let start = text.find(header).expect("branch header");

    assert_findings(
        &run(text, &configuration, &[])?,
        RuleIdentity::EmptyIf,
        &[[start, start + header.len()]],
    );

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
        )?,
        RuleIdentity::NonConstRequire,
        &[[8, 12]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[constant],
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
        )?,
        RuleIdentity::RestrictedModulePaths,
        &[[8, 19]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/requires/clear.luau"),
            &configuration,
            &[clear],
        )?,
        RuleIdentity::RestrictedModulePaths,
        &[],
    );

    Ok(())
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
            run("return 1/0", &settings, &[])?.findings[0].severity,
            severity
        );
    }

    settings
        .rules
        .get_mut(&RuleIdentity::DivideByZero)
        .expect("rule")
        .enabled = false;

    assert_findings(
        &run("return 1/0", &settings, &[])?,
        RuleIdentity::DivideByZero,
        &[],
    );

    let options = Options::new(Some(Instant::now()));

    assert_eq!(
        run_with_options("return 1", &settings, &[], &options)?.completion,
        Completion::TimedOut
    );

    let options = Options::new(Some(Instant::now() + Duration::from_secs(5)));
    options.cancellation.cancel();

    assert_eq!(
        run_with_options("return 1", &settings, &[], &options)?.completion,
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
        )?,
        RuleIdentity::PreferConst,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/table.luau"),
            &configuration,
            &[],
        )?,
        RuleIdentity::PreferConst,
        &[[6, 7]],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/protection.luau"),
            &configured(RuleIdentity::IgnoredPcallResult),
            &[],
        )?,
        RuleIdentity::IgnoredPcallResult,
        &[],
    );

    assert_findings(
        &run(
            include_str!("fixtures/shadowing/color.luau"),
            &configured(RuleIdentity::RobloxIncorrectColor3NewBounds),
            &[],
        )?,
        RuleIdentity::RobloxIncorrectColor3NewBounds,
        &[],
    );

    Ok(())
}

#[test]
fn syntax_errors_are_explicit() -> io::Result<()> {
    let text = include_str!("fixtures/syntax/input.luau");
    let result = run(text, &configured(RuleIdentity::DivideByZero), &[])?;
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
            &run(text, &configuration, &[])?,
            RuleIdentity::RobloxPreferGetPlayers,
            &[[7, text.len()]],
        );
    }

    assert_findings(
        &run(
            include_str!("fixtures/services/other.luau"),
            &configuration,
            &[],
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
        &operation,
    )?;

    assert!(events.load(Ordering::Relaxed) >= 2);
    assert_eq!(result.completion, Completion::Cancelled);
    assert_eq!(result.findings.len(), 1);
    assert_eq!(result.findings[0].code, RuleIdentity::DivideByZero.name());

    Ok(())
}
