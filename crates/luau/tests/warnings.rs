//! Native warning identities, semantic evidence and guarded session boundaries.

use std::{collections::BTreeMap, io, time::Duration};

use instar_luau::{
    BindingKind, ConfigurationEvaluator, ConfigurationKey, ConfigurationValue, FactsInput,
    ImportIdentity, ImportOutcome, ImportRequest, ImportSite, LanguageMode, LintInput, Native,
    ResolvedImport, SemanticCategory, SemanticIdentity, UpstreamWarningIdentity,
    UpstreamWarningSettings,
};

use instar_shared::{Availability, Completion, DiagnosticCategory, Knowledge, Reason, Severity};

use crate::support::{configuration, definition, location, module, options, request};

fn warnings(names: &[&str]) -> UpstreamWarningSettings {
    UpstreamWarningSettings {
        enabled: names
            .iter()
            .map(|name| UpstreamWarningIdentity {
                name: (*name).to_owned(),
            })
            .collect(),
        language_mode: LanguageMode::Nonstrict,
    }
}

#[test]
fn warning_identities_severity_and_utf8_ranges_preserve_checker_results() -> io::Result<()> {
    let source = include_str!("fixtures/warnings/warning/input.luau");
    let input = request(vec![module(1, source, 8)?])?;
    let mut native = Native::default();
    let before = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let linted = native.lint(
        &LintInput {
            analysis: before.modules[0]
                .reference
                .clone()
                .expect("established reference"),
            warnings: warnings(&["LocalUnused"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(linted.completion, Completion::Complete);
    assert_eq!(linted.prerequisite_diagnostics.as_slice(), []);
    assert_eq!(linted.findings.len(), 1);
    let warning = &linted.findings[0];
    assert_eq!(warning.warning.name, "LocalUnused");
    assert_eq!(warning.diagnostic.code, "LocalUnused");
    assert_eq!(warning.diagnostic.severity, Severity::Warning);
    assert_ne!(warning.diagnostic.message, "");

    assert_eq!(
        warning.diagnostic.primary.source,
        input.modules[0].source.source.version
    );

    assert_eq!(warning.diagnostic.primary.source.revision, 8);

    assert_eq!(
        &source[warning.diagnostic.primary.span.start..warning.diagnostic.primary.span.end],
        "unused"
    );

    assert_eq!(
        native.analyze(&input, &options(Duration::from_secs(5)))?,
        before
    );

    native.finish()
}

#[test]
fn executable_layers_directives_and_reconfiguration_control_warnings() -> io::Result<()> {
    let mut evaluator = ConfigurationEvaluator::default();

    let enabled = evaluator.evaluate(
        &configuration(
            include_str!("fixtures/warnings/executable/enabled.luau"),
            true,
        )?,
        &options(Duration::from_secs(2)),
    )?;

    let disabled = evaluator.evaluate(
        &configuration(
            include_str!("fixtures/warnings/executable/disabled.luau"),
            true,
        )?,
        &options(Duration::from_secs(2)),
    )?;

    assert_eq!(enabled.completion, Completion::Complete);
    assert_eq!(disabled.completion, Completion::Complete);

    let Some(ConfigurationValue::Table(enabled)) = enabled.values else {
        panic!("enabled table");
    };

    let Some(ConfigurationValue::Table(disabled)) = disabled.values else {
        panic!("disabled table");
    };

    let enabled = enabled
        .iter()
        .find(|entry| entry.key == ConfigurationKey::String("linting".to_owned()))
        .expect("enabled linting");

    let disabled = disabled
        .iter()
        .find(|entry| entry.key == ConfigurationKey::String("linting".to_owned()))
        .expect("disabled linting");

    assert!(
        matches!(&enabled.value, ConfigurationValue::Table(entries) if entries.iter().any(|entry| entry.key == ConfigurationKey::String("enabled".to_owned()) && entry.value == ConfigurationValue::Boolean(true)))
    );

    assert!(
        matches!(&disabled.value, ConfigurationValue::Table(entries) if entries.iter().any(|entry| entry.key == ConfigurationKey::String("enabled".to_owned()) && entry.value == ConfigurationValue::Boolean(false)))
    );

    let mut native = Native::default();

    for (source, revision, selected, count) in [
        (
            include_str!("fixtures/warnings/executable/input.luau"),
            1,
            vec!["LocalUnused"],
            1,
        ),
        (
            include_str!("fixtures/warnings/executable/suppressed.luau"),
            2,
            vec!["LocalUnused"],
            0,
        ),
        (
            include_str!("fixtures/warnings/executable/input.luau"),
            3,
            Vec::new(),
            0,
        ),
        (
            include_str!("fixtures/warnings/executable/input.luau"),
            4,
            vec!["LocalUnused"],
            1,
        ),
    ] {
        let established = native.analyze(
            &request(vec![module(1, source, revision)?])?,
            &options(Duration::from_secs(5)),
        )?;

        let linted = native.lint(
            &LintInput {
                analysis: established.modules[0]
                    .reference
                    .clone()
                    .expect("established reference"),
                warnings: warnings(&selected),
            },
            &options(Duration::from_secs(5)),
        )?;

        assert_eq!(linted.findings.len(), count);

        assert!(
            linted
                .findings
                .iter()
                .all(|finding| finding.diagnostic.primary.source.revision == revision)
        );
    }

    evaluator.finish()?;

    native.finish()
}

#[test]
fn typed_native_warnings_use_the_shared_type_graph() -> io::Result<()> {
    let source = include_str!("fixtures/warnings/typed/input.luau");
    let input = request(vec![module(1, source, 5)?])?;
    let mut native = Native::default();
    let checked = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let linted = native.lint(
        &LintInput {
            analysis: checked.modules[0]
                .reference
                .clone()
                .expect("established reference"),
            warnings: warnings(&["TableOperations"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(linted.completion, Completion::Complete);

    let warning = linted
        .findings
        .iter()
        .find(|finding| finding.warning.name == "TableOperations")
        .expect("typed table operation warning");

    assert_eq!(warning.diagnostic.code, "TableOperations");
    assert_eq!(warning.diagnostic.severity, Severity::Warning);

    assert_eq!(
        &source[warning.diagnostic.primary.span.start..warning.diagnostic.primary.span.end],
        "#record"
    );

    assert_eq!(
        native.analyze(&input, &options(Duration::from_secs(5)))?,
        checked
    );

    native.finish()
}

#[test]
fn host_dependency_contexts_retain_their_warning_revisions() -> io::Result<()> {
    let source = include_str!("fixtures/warnings/host/input.luau");
    let mut entry = module(1, source, 2)?;
    let dependency = module(2, include_str!("fixtures/warnings/host/dependency.luau"), 9)?;
    let call = source.find("loader(").expect("call");
    let argument = source.find("'./").expect("argument");

    entry.imports.push(ResolvedImport {
        site: ImportSite {
            identity: ImportIdentity(1),
            call: location(&entry, [call, source.len()])?,
            argument: location(&entry, [argument, source.len() - 1])?,
            request: ImportRequest::Static(source[argument + 1..source.len() - 2].to_owned()),
        },
        outcome: ImportOutcome::Module(dependency.analysis.module),
    });

    let input = request(vec![entry, dependency])?;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    assert_eq!(
        established
            .modules
            .iter()
            .map(|module| module.analysis.module)
            .collect::<Vec<_>>(),
        input
            .modules
            .iter()
            .map(|module| module.analysis.module)
            .collect::<Vec<_>>()
    );

    let mut findings = Vec::new();

    for module in &established.modules {
        let linted = native.lint(
            &LintInput {
                analysis: module.reference.clone().expect("established reference"),
                warnings: warnings(&["LocalUnused"]),
            },
            &options(Duration::from_secs(5)),
        )?;

        assert_eq!(linted.completion, Completion::Complete);
        findings.extend(linted.findings);
    }

    assert_eq!(findings.len(), 1);

    assert_eq!(
        findings[0].diagnostic.primary.source,
        input.modules[1].source.source.version
    );

    assert_eq!(findings[0].diagnostic.primary.source.revision, 9);

    let facts = native.facts(
        &FactsInput {
            analysis: established.modules[0]
                .reference
                .clone()
                .expect("entry reference"),
            categories: vec![SemanticCategory::Bindings],
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(facts.completion, Completion::Complete);

    native.finish()
}

#[test]
fn invalid_definitions_retain_available_syntax_warnings_and_diagnostics() -> io::Result<()> {
    let mut entry = module(1, include_str!("fixtures/warnings/invalid/input.luau"), 1)?;

    let broken = definition(
        2,
        include_str!("fixtures/warnings/invalid/invalid.luau"),
        3,
        "@custom",
    )?;

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions = vec![broken.clone()];

    let mut native = Native::default();

    let established = native.analyze(
        &request(vec![entry.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(
        established.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    let linted = native.lint(
        &LintInput {
            analysis: established.modules[0]
                .reference
                .clone()
                .expect("parsed reference"),
            warnings: warnings(&["LocalUnused", "TableLiteral"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert!(
        linted
            .prerequisite_diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.primary.source == broken.source.version
                    && diagnostic.primary.source.revision == 3
                    && diagnostic.category == DiagnosticCategory::Syntax
            )
    );

    assert!(
        linted
            .findings
            .iter()
            .any(|finding| finding.warning.name == "LocalUnused")
    );

    assert!(
        linted
            .findings
            .iter()
            .any(|finding| finding.warning.name == "TableLiteral")
    );

    entry.analysis.revision = 2;

    entry
        .environment
        .as_mut()
        .expect("environment")
        .declarations[0]
        .definitions
        .clear();

    let established = native.analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))?;

    assert_eq!(
        native
            .lint(
                &LintInput {
                    analysis: established.modules[0]
                        .reference
                        .clone()
                        .expect("restored reference"),
                    warnings: warnings(&["LocalUnused", "TableLiteral"])
                },
                &options(Duration::from_secs(5))
            )?
            .completion,
        Completion::Complete
    );

    native.finish()
}

#[test]
fn inferred_any_facts_exclude_annotations_and_inferred_numbers() -> io::Result<()> {
    let source = include_str!("fixtures/warnings/inferred/input.luau");
    let input = request(vec![module(1, source, 6)?])?;
    let mut native = Native::default();
    let checked = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let facts = native.facts(
        &FactsInput {
            analysis: checked.modules[0]
                .reference
                .clone()
                .expect("established reference"),
            categories: vec![SemanticCategory::Bindings],
        },
        &options(Duration::from_secs(5)),
    )?;

    let candidates = facts.facts.bindings.iter().filter(|binding| !binding.annotation && matches!(binding.kind, BindingKind::Local | BindingKind::Parameter) && matches!(&binding.type_information, Knowledge::Known(information) if information.is_any)).collect::<Vec<_>>();
    assert_eq!(candidates.len(), 2);

    let expected: BTreeMap<String, [usize; 2]> =
        serde_json::from_str(include_str!("fixtures/warnings/inferred/expected.json"))?;

    for (name, kind) in [
        ("inferred", BindingKind::Local),
        ("parameter", BindingKind::Parameter),
    ] {
        let binding = candidates
            .iter()
            .find(|binding| binding.name == name)
            .expect("implicit any binding");

        assert_eq!(binding.kind, kind);

        let SemanticIdentity::Source { location, .. } = binding.identity else {
            panic!("source binding identity");
        };

        assert_eq!([location.span.start, location.span.end], expected[name]);
        assert_eq!(&source[location.span.start..location.span.end], name);
        assert_eq!(location.source.revision, 6);
    }

    assert_eq!(
        native.analyze(&input, &options(Duration::from_secs(5)))?,
        checked
    );

    native.finish()
}

#[test]
fn generic_for_any_facts_preserve_binding_identity_and_exclusions() -> io::Result<()> {
    let source = include_str!("fixtures/warnings/loops/input.luau");
    let input = request(vec![module(1, source, 7)?])?;
    let mut native = Native::default();
    let checked = native.analyze(&input, &options(Duration::from_secs(5)))?;
    assert_eq!(checked.modules[0].diagnostics.as_slice(), []);

    let query = FactsInput {
        analysis: checked.modules[0]
            .reference
            .clone()
            .expect("established reference"),
        categories: vec![SemanticCategory::Bindings],
    };

    let facts = native.facts(&query, &options(Duration::from_secs(5)))?;
    assert_eq!(facts.completion, Completion::Complete);
    assert_eq!(facts.diagnostics.as_slice(), []);

    let expected: Vec<[usize; 2]> =
        serde_json::from_str(include_str!("fixtures/warnings/loops/expected.json"))?;

    assert_eq!(expected.len(), 2);
    let candidates = facts.facts.bindings.iter().filter(|binding| !binding.annotation && binding.kind == BindingKind::Loop && matches!(&binding.type_information, Knowledge::Known(information) if information.is_any)).collect::<Vec<_>>();
    assert_eq!(candidates.len(), 2);

    let ranges = candidates
        .iter()
        .map(|binding| {
            let SemanticIdentity::Source { location, .. } = binding.identity else {
                panic!("source loop identity");
            };

            assert_eq!(location.source, input.modules[0].source.source.version);
            assert_eq!(location.source.revision, 7);
            assert_eq!(&source[location.span.start..location.span.end], "value");

            [location.span.start, location.span.end]
        })
        .collect::<Vec<_>>();

    assert_eq!(ranges, expected);

    assert_eq!(
        native.analyze(&input, &options(Duration::from_secs(5)))?,
        checked
    );

    assert_eq!(
        native.facts(&query, &options(Duration::from_secs(5)))?,
        facts
    );

    native.finish()
}

#[test]
fn nocheck_and_interruption_have_explicit_completion_without_stale_warnings() -> io::Result<()> {
    let mut entry = module(1, include_str!("fixtures/warnings/nocheck/input.luau"), 1)?;
    entry.inference.required = false;
    entry.inference.retained.clear();
    entry.reporting.ordinary_type_diagnostics = false;
    entry.environment = None;
    let input = request(vec![entry])?;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("parsed reference");

    assert_eq!(
        native
            .lint(
                &LintInput {
                    analysis: reference.clone(),
                    warnings: warnings(&["LocalUnused"])
                },
                &options(Duration::from_secs(5))
            )?
            .completion,
        Completion::Complete
    );

    let semantic = native.facts(
        &FactsInput {
            analysis: reference.clone(),
            categories: vec![SemanticCategory::Bindings],
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(
        semantic.completion,
        Completion::Incomplete(vec![Reason::UnavailableSemantics])
    );

    assert_eq!(semantic.facts.bindings.as_slice(), []);

    assert_eq!(
        semantic.availability.categories[&SemanticCategory::Bindings],
        Availability::Unavailable(Reason::UnavailableSemantics)
    );

    let cancelled = options(Duration::from_secs(5));
    cancelled.cancellation.cancel();

    assert_eq!(
        native
            .lint(
                &LintInput {
                    analysis: reference,
                    warnings: warnings(&["LocalUnused"])
                },
                &cancelled
            )?
            .completion,
        Completion::Cancelled
    );

    assert_eq!(
        native.analyze(&input, &options(Duration::ZERO))?.completion,
        Completion::TimedOut
    );

    let retry = native.analyze(
        &request(vec![module(1, "return 1", 2)?])?,
        &options(Duration::from_secs(5)),
    )?;

    let retry = native.lint(
        &LintInput {
            analysis: retry.modules[0].reference.clone().expect("retry reference"),
            warnings: warnings(&["LocalUnused"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(retry.completion, Completion::Complete);
    assert_eq!(retry.findings.as_slice(), []);

    native.finish()
}

#[test]
fn syntax_diagnostics_are_retained_without_leaking_source_type_errors() -> io::Result<()> {
    let mut entry = module(1, include_str!("fixtures/warnings/syntax/input.luau"), 1)?;
    let mut native = Native::default();

    let checked = native.analyze(
        &request(vec![entry.clone()])?,
        &options(Duration::from_secs(5)),
    )?;

    assert!(
        checked.modules[0]
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Type)
    );

    entry.analysis.revision = 2;
    entry.reporting.ordinary_type_diagnostics = false;
    let established = native.analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))?;

    let linted = native.lint(
        &LintInput {
            analysis: established.modules[0]
                .reference
                .clone()
                .expect("lint reference"),
            warnings: warnings(&["TableOperations"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(linted.prerequisite_diagnostics.as_slice(), []);
    let mut entry = module(1, include_str!("fixtures/warnings/syntax/invalid.luau"), 2)?;
    entry.analysis.revision = 3;
    entry.reporting.ordinary_type_diagnostics = false;
    let established = native.analyze(&request(vec![entry])?, &options(Duration::from_secs(5)))?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("recovered reference");

    let malformed = native.lint(
        &LintInput {
            analysis: reference.clone(),
            warnings: warnings(&["TableOperations"]),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert!(
        malformed
            .prerequisite_diagnostics
            .iter()
            .any(|diagnostic| diagnostic.primary.source.revision == 2
                && diagnostic.category == DiagnosticCategory::Syntax)
    );

    let replacement = native.analyze(
        &request(vec![module(2, "return 1", 1)?])?,
        &options(Duration::from_secs(5)),
    )?;

    assert_ne!(replacement.modules.as_slice(), []);

    assert_eq!(
        native
            .facts(
                &FactsInput {
                    analysis: reference,
                    categories: vec![SemanticCategory::Bindings]
                },
                &options(Duration::from_secs(5))
            )
            .expect_err("semantic reference must remain established")
            .get_ref()
            .and_then(|error| error.downcast_ref::<Reason>()),
        Some(&Reason::StaleQuery)
    );

    native.finish()
}
