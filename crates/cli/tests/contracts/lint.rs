use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    slice::from_ref,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use instar_lint::{Preset, RuleConfiguration, RuleIdentity, RuleSettings};

use instar_project::{
    ApplyRequest, DiagnosticPolicy, FixMode, Input, Invocation, LintOverrides, LintPatch,
    LintReport, LintRequest, Project, WatchCommandReport, WatchOperation, WatchRequest,
    WriteAuthorization, WriteOutcome,
};

use instar_shared::{Completion, DiagnosticCategory, EventPayload, Options, Severity};
use serde_json::json;

use super::support::{Fixture, attributed, complete, diagnostic, options, snapshot};

#[test]
fn retained_pairs_preserve_inventory_ranges_and_negative_cases() {
    let cases = [
        ("swapping", "almost_swapped", [21, 24]),
        ("number", "compare_nan", [10, 18]),
        ("condition", "constant_condition", [6, 10]),
        ("comparison", "constant_table_comparison", [10, 21]),
        ("length", "length_as_condition", [6, 11]),
        ("arguments", "mismatched_argument_count", [60, 64]),
        ("result", "must_use", [3, 15]),
        ("step", "zero_step_loop", [14, 15]),
        ("division", "divide_by_zero", [12, 16]),
        ("assignment", "self_assignment", [17, 22]),
        ("branch", "empty_if", [3, 15]),
        ("loop", "empty_loop", [3, 16]),
        ("branches", "if_same_then_else", [3, 42]),
        ("protection", "ignored_pcall_result", [3, 17]),
        ("tables", "mixed_table", [10, 19]),
        ("globals", "unscoped_variables", [3, 10]),
        ("conditional", "and_or_conditional", [10, 25]),
        ("nesting", "collapsible_if", [3, 39]),
        ("return", "else_after_return", [42, 55]),
        ("expression", "if_expression_assignment", [11, 40]),
        ("negation", "negated_condition", [6, 14]),
        ("parentheses", "parenthesized_conditions", [6, 12]),
        ("usage", "global_usage", [10, 18]),
        ("types", "type_check_inside_call", [10, 22]),
        ("invariance", "loop_invariant_call", [38, 49]),
        ("cloning", "manual_table_clone", [17, 55]),
        ("concatenation", "string_concat_in_loop", [33, 39]),
        ("bindings", "unused_variable", [9, 15]),
        ("constants", "prefer_const", [9, 14]),
        ("deprecation", "deprecated", [3, 10]),
        ("restrictions", "restricted_globals", [3, 8]),
        ("escapes", "bad_string_escape", [7, 11]),
    ];

    for (scenario, rule, range) in cases {
        let fixture = Fixture::new("lint", scenario);

        let setting = match scenario {
            "deprecation" => Some(
                r#"linting.rules.deprecated={"enabled":true,"severity":"Warning","settings":{"rule":"deprecated","parameters":{"paths":{"old.api":"new.api"}}}}"#,
            ),

            "restrictions" => Some(
                r#"linting.rules.restricted_globals={"enabled":true,"severity":"Warning","settings":{"rule":"restricted_globals","parameters":{"names":{"print":"use logging"}}}}"#,
            ),

            _ => None,
        };

        let mut arguments = vec!["lint", "positive.luau", "--rule", rule];
        let mut request = prepare(&fixture, &["positive.luau"], &[rule]);

        if let Some(setting) = setting {
            arguments.extend(["--set", setting]);

            request.overrides.linting.rules = Some(BTreeMap::from([(
                RuleIdentity::named(rule).expect("known rule"),
                serde_json::from_str(setting.split_once('=').expect("setting value").1)
                    .expect("typed rule configuration"),
            )]));
        }

        let report = lint(&fixture, request);
        let original = fixture.bytes("positive.luau");
        let output = fixture.run(&arguments);
        assert_eq!(output.status, 0, "{scenario}: {}", output.stderr);

        assert_eq!(
            output.stderr.matches(&format!("warning[{rule}]")).count(),
            1,
            "{scenario}: {}",
            output.stderr
        );

        assert_eq!(
            report.completion,
            Completion::Complete,
            "{scenario}: {report:?}"
        );

        let diagnostic = diagnostic(
            &report.final_findings,
            &report.snapshot,
            rule,
            "positive.luau",
            range,
        );

        assert_eq!(diagnostic.severity, Severity::Warning);
        assert_eq!(diagnostic.category, DiagnosticCategory::Lint);

        let count = report
            .final_findings
            .iter()
            .filter(|finding| finding.code == rule)
            .count();

        assert_eq!(count, 1, "negative case must not diagnose: {scenario}");
        assert_eq!(fixture.bytes("positive.luau"), original);

        if scenario == "arguments" || scenario == "escapes" || scenario == "invariance" {
            let negative = fixture.run(&["lint", "negative.luau", "--rule", rule]);
            assert_eq!(negative.status, 0, "{scenario}: {}", negative.stderr);
            assert!(!negative.stderr.contains(&format!("warning[{rule}]")));

            assert_eq!(
                lint(&fixture, prepare(&fixture, &["negative.luau"], &[rule])).final_findings,
                []
            );
        }
    }
}

#[test]
fn native_warning_programs_have_attributed_positive_and_negative_findings() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write(".luaurc", r#"{"languageMode":"nocheck"}"#);

    let cases = [
        ("UnknownGlobal", "missing", 0),
        ("GlobalUsedAsLocal", "created", 0),
        ("LocalShadow", "shadow", 1),
        ("SameLineStatement", "print(2)", 0),
        ("MultiLineStatement", "2\n    print(continued)", 0),
        ("LocalUnused", "unused", 0),
        ("FunctionUnused", "unusedfunction", 0),
        ("ImportUnused", "module", 0),
        ("BuiltinGlobalWrite", "math", 0),
        ("PlaceholderRead", "_)", 0),
        ("UnreachableCode", "print('unreachable')", 0),
        ("UnknownType", "'integer'", 0),
        ("ForRange", "8, 1", 0),
        ("UnbalancedAssignment", "local first, second = 1", 0),
        ("ImplicitReturn", "if flag then return 1 end", 0),
        ("DuplicateLocal", "duplicate", 1),
        ("FormatString", "'%qz%'", 0),
        ("TableLiteral", "key", 1),
        ("UninitializedLocal", "uninitialized", 1),
        ("DuplicateFunction", "object.method", 1),
        ("DeprecatedApi", "getfenv(1)", 0),
        ("TableOperations", "0, 3", 0),
        ("DuplicateCondition", "flag then print(2)", 0),
        ("MisleadingAndOr", "flag and false or 2", 0),
        ("IntegerParsing", "9007199254740993", 0),
        ("ComparisonPrecedence", "not first == second", 0),
    ];

    let source = fixture.read("positive.luau");
    let mut reports = Vec::new();

    for (rule, text, occurrence) in cases {
        let start = source
            .match_indices(text)
            .nth(occurrence)
            .expect("literal warning input")
            .0;

        let length = match rule {
            "MultiLineStatement" | "TableOperations" | "PlaceholderRead" => 1,
            "DuplicateCondition" => 4,
            _ => text.len(),
        };

        let request = prepare(&fixture, &["positive.luau"], &[rule]);
        let report = lint(&fixture, request);
        assert!(!report.failed, "{rule}: {report:?}");

        assert_eq!(
            report.completion,
            Completion::Complete,
            "{rule}: {report:?}"
        );

        let finding = diagnostic(
            &report.final_findings,
            &report.snapshot,
            rule,
            "positive.luau",
            [start, start + length],
        );

        assert_eq!(finding.severity, Severity::Warning);
        assert_eq!(finding.category, DiagnosticCategory::Lint);
        assert_ne!(finding.message, "");
        let request = prepare(&fixture, &["negative.luau"], &[rule]);
        let negative_report = lint(&fixture, request);
        assert!(!negative_report.failed, "{rule}: {negative_report:?}");
        assert_eq!(negative_report.final_findings, [], "{rule}");

        reports.push(fixture.normalize(&json!({
            "rule": rule, "findings": attributed(&report.final_findings, &report.snapshot), "sources": report.sources,
            "availability": report.availability, "counts": report.counts,
            "completion": report.completion,
        })));
    }

    for (rule, name, range, clean) in [
        (
            "CommentDirective",
            "directive.luau",
            [0, 10],
            "negative.luau",
        ),
        (
            "RedundantNativeAttribute",
            "native.luau",
            [10, 17],
            "attribute.luau",
        ),
    ] {
        let request = prepare(&fixture, &[name], &[rule]);
        let report = lint(&fixture, request);
        assert!(!report.failed, "{report:?}");

        diagnostic(&report.final_findings, &report.snapshot, rule, name, range);
        let request = prepare(&fixture, &[clean], &[rule]);
        let negative_report = lint(&fixture, request);
        assert!(!negative_report.failed, "{negative_report:?}");
        assert_eq!(negative_report.final_findings, []);

        reports.push(fixture.normalize(&json!({
            "rule": rule, "findings": attributed(&report.final_findings, &report.snapshot), "sources": report.sources,
            "availability": report.availability, "counts": report.counts,
            "completion": report.completion,
        })));
    }

    snapshot(&fixture, "lint", "warnings", &json!(reports));
}

#[test]
fn resolved_signatures_obey_required_optional_method_and_variadic_parameters() {
    let fixture = Fixture::new("cli", "lint");
    let source = fixture.read("calls.luau");
    let mut reports = Vec::new();

    for mode in ["strict", "nonstrict", "nocheck"] {
        fixture.write(".luaurc", json!({"languageMode": mode}).to_string());

        let mut request = prepare(&fixture, &["calls.luau"], &["mismatched_argument_count"]);
        request.overrides.selection.exclude = Some(vec!["module.luau".to_owned()]);

        let report = lint(&fixture, request);

        assert!(!report.failed, "{mode}: {report:?}");

        assert_eq!(
            report.completion,
            Completion::Complete,
            "{mode}: {report:?}"
        );

        for call in [
            "required(1)",
            "required(1, 2, 3)",
            "optional()",
            "module:method()",
            "module.optional(1, 2, 3)",
        ] {
            let start = source.find(call).expect("literal call");

            diagnostic(
                &report.final_findings,
                &report.snapshot,
                "mismatched_argument_count",
                "calls.luau",
                [start, start + call.len()],
            );
        }

        assert_eq!(report.final_findings.len(), 5);
        assert_eq!(report.counts.selected, 1);
        assert_eq!(report.counts.dependencies, 1);

        assert!(
            report
                .final_findings
                .iter()
                .all(|finding| finding.category != DiagnosticCategory::Type)
        );

        let request = prepare(&fixture, &["valid.luau"], &["mismatched_argument_count"]);
        let valid_report = lint(&fixture, request);

        assert!(!valid_report.failed, "{mode}: {valid_report:?}");
        assert_eq!(valid_report.completion, Completion::Complete);
        assert_eq!(valid_report.final_findings, []);

        reports.push(fixture.normalize(&json!({
            "mode": mode, "findings": report.final_findings, "sources": report.sources,
            "availability": report.availability, "counts": report.counts,
            "completion": report.completion,
        })));
    }

    snapshot(&fixture, "lint", "signatures", &json!(reports));
}

#[test]
fn declarations_inference_and_type_functions_survive_source_type_errors() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write("instar.toml", "[linting]\npresets = ['none']\n[[environment.groups]]\nnamespace = '@project'\ndefinitions = ['declaration.luau']\n");
    let source = fixture.read("globals.luau");
    let mut reports = Vec::new();

    for mode in ["strict", "nonstrict", "nocheck"] {
        fixture.write(".luaurc", json!({"languageMode": mode}).to_string());

        let request = prepare(
            &fixture,
            &["globals.luau"],
            &["implicit_any_local", "implicit_any_parameter"],
        );

        let report = lint(&fixture, request);

        assert!(!report.failed, "{mode}: {report:?}");

        assert_eq!(
            report.completion,
            Completion::Complete,
            "{mode}: {report:?}"
        );

        for (rule, text) in [
            ("implicit_any_local", "inferred"),
            ("implicit_any_parameter", "parameter"),
        ] {
            let start = source.find(text).expect("literal binding");

            diagnostic(
                &report.final_findings,
                &report.snapshot,
                rule,
                "globals.luau",
                [start, start + text.len()],
            );
        }

        assert_eq!(report.final_findings.len(), 2);

        reports.push(fixture.normalize(&json!({
            "mode": mode, "findings": report.final_findings, "sources": report.sources,
            "availability": report.availability, "counts": report.counts,
            "completion": report.completion,
        })));
    }

    let request = prepare(&fixture, &["function.luau"], &["mismatched_argument_count"]);

    let report = lint(&fixture, request);

    assert!(!report.failed, "{report:?}");

    let source = fixture.read("function.luau");
    let start = source.find("consume()").expect("typed call");

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "mismatched_argument_count",
        "function.luau",
        [start, start + "consume()".len()],
    );

    assert_eq!(report.completion, Completion::Complete);

    reports.push(fixture.normalize(&json!({
        "case": "functions", "findings": report.final_findings, "sources": report.sources,
        "availability": report.availability, "counts": report.counts,
        "completion": report.completion,
    })));

    snapshot(&fixture, "lint", "inference", &json!(reports));
}

#[test]
fn recovery_and_missing_prerequisites_retain_useful_findings() {
    let fixture = Fixture::new("cli", "lint");

    let request = prepare(&fixture, &["recovered.luau"], &["divide_by_zero"]);
    let output = fixture.run(&["lint", "recovered.luau", "--rule", "divide_by_zero"]);
    let report = lint(&fixture, request);

    assert_ne!(output.status, 0);

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "divide_by_zero",
        "recovered.luau",
        [19, 20],
    );

    assert!(
        report
            .final_findings
            .iter()
            .any(|finding| finding.category == DiagnosticCategory::Syntax)
    );

    assert!(fixture.read("recovered.luau").contains("local ="));

    fixture.write("instar.toml", "[linting]\npresets = ['none']\n[[environment.groups]]\nnamespace = '@broken'\ndefinitions = ['invalid.luau']\n");

    let request = prepare(
        &fixture,
        &["globals.luau"],
        &["implicit_any_local", "divide_by_zero"],
    );

    let output = fixture.run(&[
        "lint",
        "globals.luau",
        "--rule",
        "implicit_any_local",
        "--rule",
        "divide_by_zero",
    ]);

    let report = lint(&fixture, request);

    assert_ne!(output.status, 0);

    assert_ne!(report.completion, Completion::Complete);

    assert!(report.final_findings.iter().any(|finding| {
        report
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            .is_some_and(|path| path.ends_with("invalid.luau"))
    }));

    assert!(!report.availability.is_empty());
}

#[test]
fn suppression_scopes_and_mistakes_have_precise_locations() {
    let fixture = Fixture::new("cli", "lint");

    let request = prepare(&fixture, &["suppression.luau"], &["divide_by_zero"]);
    let output = fixture.run(&["lint", "suppression.luau", "--rule", "divide_by_zero"]);
    let report = lint(&fixture, request);

    assert_eq!(output.status, 1, "{}", output.stderr);

    let source = fixture.read("suppression.luau");

    for expression in ["3 / 0", "4 / 0"] {
        let start = source.find(expression).expect("visible expression") + 4;

        diagnostic(
            &report.final_findings,
            &report.snapshot,
            "divide_by_zero",
            "suppression.luau",
            [start, start + 1],
        );
    }

    for (directive, message) in [
        (
            "-- instar-lint ignore nonexistent",
            "unknown lint rule nonexistent",
        ),
        (
            "-- instar-lint ignore divide_by_zero\nlocal ineffective",
            "ineffective suppression for divide_by_zero",
        ),
    ] {
        let start = source.find(directive).expect("directive input");
        let end = start + directive.lines().next().expect("comment").len();

        let finding = diagnostic(
            &report.final_findings,
            &report.snapshot,
            "lint.directive",
            "suppression.luau",
            [start, end],
        );

        assert_eq!(finding.message, message);
    }

    assert_eq!(report.final_findings.len(), 4);
    let request = prepare(&fixture, &["file.luau"], &["divide_by_zero"]);
    let output = fixture.run(&["lint", "file.luau", "--rule", "divide_by_zero"]);
    let report = lint(&fixture, request);
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(report.final_findings, []);
}

#[test]
fn rule_policy_presets_thresholds_and_filters_remain_independent() {
    let fixture = Fixture::new("lint", "bindings");
    let disabled = fixture.run(&["lint", "positive.luau", "--preset", "none"]);
    assert_eq!(disabled.status, 0);
    assert!(!disabled.stderr.contains("warning[unused_variable]"));

    for (severity, threshold, label, status) in [
        ("Information", "information", "note", 1),
        ("Warning", "error", "warning", 0),
        ("Error", "error", "error", 1),
    ] {
        let setting = format!(
            r#"linting.rules.unused_variable={{"enabled":true,"severity":"{severity}","settings":{{"rule":"unused_variable","parameters":{{}}}}}}"#
        );

        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--rule",
            "unused_variable",
            "--set",
            &setting,
            "--failure-threshold",
            threshold,
        ]);

        assert_eq!(output.status, status, "{}", output.stderr);

        assert!(output.stderr.contains(&format!("{label}[unused_variable]")));
    }

    for filter in [
        ["--diagnostic-code", "must_use"],
        ["--diagnostic-category", "syntax"],
        ["--diagnostic-severity", "error"],
    ] {
        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--rule",
            "unused_variable",
            "--warnings-as-errors",
            filter[0],
            filter[1],
        ]);

        assert_eq!(
            output.status, 1,
            "display filters cannot change failure: {}",
            output.stderr
        );

        assert!(!output.stderr.contains("warning[unused_variable]"));
    }

    let duplicate = fixture.run(&[
        "lint",
        "positive.luau",
        "--rule",
        "unused_variable",
        "--rule",
        "LocalUnused",
    ]);

    assert_eq!(duplicate.status, 0);
    assert_eq!(duplicate.stderr.matches("warning[LocalUnused]").count(), 1);
    assert!(!duplicate.stderr.contains("warning[unused_variable]"));

    let fixture = Fixture::new("lint", "shadowing");
    let setting = r#"linting.rules.unused_variable={"enabled":true,"severity":"Warning","settings":{"rule":"unused_variable","parameters":{"parameters":true,"loop_variables":true,"ignore_pattern":"^skip$"}}}"#;

    let output = fixture.run(&[
        "lint",
        "input.luau",
        "--rule",
        "unused_variable",
        "--set",
        setting,
    ]);

    assert_eq!(output.status, 0);
    assert_eq!(output.stderr.matches("warning[unused_variable]").count(), 2);

    let fixture = Fixture::new("lint", "constants");

    for (name, expected) in [
        ("none", 0),
        ("correctness", 0),
        ("recommended", 1),
        ("all", 1),
    ] {
        let output = fixture.run(&["lint", "positive.luau", "--preset", name]);
        assert_eq!(output.status, 0, "{name}: {}", output.stderr);

        assert_eq!(
            output.stderr.matches("warning[prefer_const]").count(),
            expected,
            "{name}: {}",
            output.stderr
        );
    }

    let setting = r#"linting.rules.prefer_const={"enabled":false,"severity":"Warning","settings":{"rule":"prefer_const","parameters":{}}}"#;
    let disabled = fixture.run(&["lint", "positive.luau", "--preset", "all", "--set", setting]);
    assert_eq!(disabled.status, 0);
    assert!(!disabled.stderr.contains("warning[prefer_const]"));
}

#[test]
fn typed_rule_policy_presets_thresholds_and_filters_remain_independent() {
    let fixture = Fixture::new("lint", "bindings");
    let mut disabled = prepare(&fixture, &["positive.luau"], &[]);
    disabled.overrides.linting.presets = Some(vec![Preset::None]);
    assert_eq!(lint(&fixture, disabled).final_findings, []);

    for (severity, threshold, failed) in [
        (Severity::Information, Severity::Information, true),
        (Severity::Warning, Severity::Error, false),
        (Severity::Error, Severity::Error, true),
    ] {
        let mut invocation = prepare(&fixture, &["positive.luau"], &["unused_variable"]);
        let mut configuration = RuleConfiguration::new(RuleIdentity::UnusedVariable);
        configuration.enabled = true;
        configuration.severity = severity;

        invocation.overrides.linting.rules = Some(BTreeMap::from([(
            RuleIdentity::UnusedVariable,
            configuration,
        )]));

        invocation.overrides.diagnostics.threshold = threshold;
        let report = lint(&fixture, invocation);

        assert_eq!(
            diagnostic(
                &report.final_findings,
                &report.snapshot,
                "unused_variable",
                "positive.luau",
                [9, 15]
            )
            .severity,
            severity
        );

        assert_eq!(report.failed, failed);
    }

    for policy in [
        DiagnosticPolicy {
            codes: Some(BTreeSet::from(["must_use".to_owned()])),
            ..DiagnosticPolicy::default()
        },
        DiagnosticPolicy {
            categories: Some(BTreeSet::from([DiagnosticCategory::Syntax])),
            ..DiagnosticPolicy::default()
        },
        DiagnosticPolicy {
            severities: Some(BTreeSet::from([Severity::Error])),
            ..DiagnosticPolicy::default()
        },
    ] {
        let mut invocation = prepare(&fixture, &["positive.luau"], &["unused_variable"]);
        invocation.overrides.diagnostics = policy;
        invocation.overrides.diagnostics.warnings_as_errors = true;
        let report = lint(&fixture, invocation);
        assert_eq!(report.final_findings, []);
        assert!(report.failed);
        assert_eq!(report.completion, Completion::Complete);
    }

    let report = lint(
        &fixture,
        prepare(
            &fixture,
            &["positive.luau"],
            &["unused_variable", "LocalUnused"],
        ),
    );

    assert_eq!(report.final_findings.len(), 1);
    let fixture = Fixture::new("lint", "shadowing");
    let mut invocation = prepare(&fixture, &["input.luau"], &["unused_variable"]);
    let mut configuration = RuleConfiguration::new(RuleIdentity::UnusedVariable);

    let RuleSettings::UnusedVariable(parameters) = &mut configuration.settings else {
        panic!("unused binding parameters");
    };

    parameters.parameters = true;
    parameters.loop_variables = true;
    parameters.ignore_pattern = "^skip$".to_owned();

    invocation.overrides.linting.rules = Some(BTreeMap::from([(
        RuleIdentity::UnusedVariable,
        configuration,
    )]));

    let report = lint(&fixture, invocation);
    let source = fixture.read("input.luau");

    for text in ["value", "kept"] {
        let start = source.find(text).expect("unread declaration");

        diagnostic(
            &report.final_findings,
            &report.snapshot,
            "unused_variable",
            "input.luau",
            [start, start + text.len()],
        );
    }

    assert_eq!(report.final_findings.len(), 2);
    let fixture = Fixture::new("lint", "constants");

    for (preset, expected) in [
        (Preset::None, 0),
        (Preset::Correctness, 0),
        (Preset::Recommended, 1),
        (Preset::All, 1),
    ] {
        let mut invocation = prepare(&fixture, &["positive.luau"], &[]);
        invocation.overrides.linting.presets = Some(vec![preset]);
        let report = lint(&fixture, invocation);

        assert_eq!(
            report
                .final_findings
                .iter()
                .filter(|finding| finding.code == "prefer_const")
                .count(),
            expected,
            "{preset:?}"
        );
    }

    let mut invocation = prepare(&fixture, &["positive.luau"], &[]);
    invocation.overrides.linting.presets = Some(vec![Preset::All]);
    let mut configuration = RuleConfiguration::new(RuleIdentity::PreferConst);
    configuration.enabled = false;

    invocation.overrides.linting.rules =
        Some(BTreeMap::from([(RuleIdentity::PreferConst, configuration)]));

    assert!(
        lint(&fixture, invocation)
            .final_findings
            .iter()
            .all(|finding| finding.code != "prefer_const")
    );
}

#[test]
fn explicit_preview_write_and_scoped_fixes_use_final_source_outcomes() {
    let inputs = Fixture::new("cli", "lint");
    let fixture = Fixture::empty();

    for name in ["instar.toml", "fixing.luau", "fixed.luau"] {
        fixture.write(name, inputs.bytes(name));
    }

    let original = fixture.bytes("fixing.luau");

    let arguments = [
        "lint",
        "fixing.luau",
        "--rule",
        "self_assignment",
        "--rule",
        "parenthesized_conditions",
        "--warnings-as-errors",
    ];

    let mut invocation = prepare(
        &fixture,
        &["fixing.luau"],
        &["self_assignment", "parenthesized_conditions"],
    );

    invocation.overrides.diagnostics.warnings_as_errors = true;
    let report = lint(&fixture, invocation.clone());
    let analysis = fixture.run(&arguments);
    assert_eq!(analysis.status, 1);
    assert_eq!(fixture.bytes("fixing.luau"), original);
    assert_eq!(report.fixes.len(), 2);
    assert!(report.writes.is_empty());

    let mut preview_arguments = arguments.to_vec();
    preview_arguments.push("--fix-preview");
    invocation.fixes = FixMode::Preview;
    let report = lint(&fixture, invocation.clone());
    let preview = fixture.run(&preview_arguments);
    assert_eq!(preview.status, 1, "{}", preview.stderr);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(fixture.bytes("fixing.luau"), original);
    assert_eq!(report.proposals.len(), 1);
    assert_ne!(report.previews[0], "");
    assert!(preview.stderr.contains("---"));
    assert!(preview.stderr.contains("+++"));

    let mut write_arguments = arguments.to_vec();
    write_arguments.extend(["--fix", "--fix-rule", "self_assignment"]);
    invocation.fixes = FixMode::Write;
    invocation.fix_rules = Some(BTreeSet::from([RuleIdentity::SelfAssignment]));
    let report = lint(&fixture, invocation.clone());
    fixture.write("fixing.luau", &original);
    let scoped = fixture.run(&write_arguments);

    assert_eq!(
        scoped.status, 1,
        "unfixed warning still fails: {}",
        scoped.stderr
    );

    assert!(!fixture.read("fixing.luau").contains("value = value"));
    assert!(fixture.read("fixing.luau").contains("if (flag) then"));
    assert_eq!(report.counts.fixes, 1);
    assert_eq!(report.final_findings.len(), 1);
    assert!(scoped.stderr.contains("parenthesized_conditions"));

    fixture.write("fixing.luau", &original);
    invocation.fix_rules = None;
    let report = lint(&fixture, invocation);
    fixture.write("fixing.luau", &original);
    write_arguments.truncate(arguments.len());
    write_arguments.push("--fix");
    let fixed = fixture.run(&write_arguments);

    assert_eq!(
        fixed.status, 0,
        "final source determines outcome: {}",
        fixed.stderr
    );

    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.final_findings, []);
    assert_eq!(report.counts.fixes, 2);
    assert_eq!(report.counts.written, 1);
    assert_eq!(fixture.read("fixing.luau"), fixture.read("fixed.luau"));
}

#[test]
fn multifile_fixes_and_stdin_never_write_contextual_paths() {
    let inputs = Fixture::new("cli", "lint");
    let fixture = Fixture::empty();

    for name in ["instar.toml", "fixing.luau", "fixed.luau"] {
        fixture.write(name, inputs.bytes(name));
    }

    let original = fixture.bytes("fixing.luau");
    fixture.write("second.luau", &original);

    let mut invocation = prepare(
        &fixture,
        &["fixing.luau", "second.luau"],
        &["self_assignment", "parenthesized_conditions"],
    );

    invocation.fixes = FixMode::Write;
    invocation.overrides.diagnostics.warnings_as_errors = true;
    let report = lint(&fixture, invocation);
    assert_eq!(report.counts.written, 2);
    assert_eq!(report.counts.fixes, 4);
    fixture.write("fixing.luau", &original);
    fixture.write("second.luau", &original);

    let output = fixture.run(&[
        "lint",
        "fixing.luau",
        "second.luau",
        "--rule",
        "self_assignment",
        "--rule",
        "parenthesized_conditions",
        "--fix",
        "--warnings-as-errors",
    ]);

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(fixture.read("fixing.luau"), fixture.read("fixed.luau"));
    assert_eq!(fixture.read("second.luau"), fixture.read("fixed.luau"));
    fixture.write("fixing.luau", &original);
    let source = String::from_utf8(original.clone()).expect("Luau source");

    let mut invocation = prepare(
        &fixture,
        &[],
        &["self_assignment", "parenthesized_conditions"],
    );

    invocation.invocation.inputs = vec![Input::Stdin {
        text: Arc::from(source.as_str()),
        context: Some(fixture.path("fixing.luau")),
    }];

    invocation.fixes = FixMode::Stdin;
    invocation.overrides.diagnostics.warnings_as_errors = true;
    let report = lint(&fixture, invocation);

    let output = fixture.stdin(
        &[
            "lint",
            "-",
            "--stdin-filepath",
            "fixing.luau",
            "--rule",
            "self_assignment",
            "--rule",
            "parenthesized_conditions",
            "--fix-stdin",
            "--output",
            "report.txt",
            "--warnings-as-errors",
        ],
        &source,
    );

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.stdout, fixture.read("fixed.luau"));
    assert_eq!(fixture.bytes("fixing.luau"), original);
    assert!(fixture.read("report.txt").contains("Linting complete:"));
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.written, 0);
    assert_eq!(report.final_findings, []);
}

#[test]
fn selection_combines_inputs_explains_exclusions_and_rejects_missing_targets() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write("instar.toml", "[linting]\nselected = ['self_assignment']\n[selection.linting]\ninclude = ['selected/**']\nexclude = ['selected/excluded.luau']\n");
    let source = fixture.read("fixing.luau");

    for name in [
        "selected/one.luau",
        "selected/two.lua",
        "selected/nested/three.luau",
        "selected/excluded.luau",
        "selected/ignored.luau",
        "other.luau",
    ] {
        fixture.write(name, &source);
    }

    fixture.write(".ignore", "selected/ignored.luau\n");
    let default = fixture.run(&["lint", "--explain-selection"]);
    assert_eq!(default.status, 0, "{}", default.stderr);
    assert!(default.stderr.contains("Selection:"));
    let mut invocation = prepare(&fixture, &[], &[]);
    invocation.invocation.explanations.selection = true;
    let report = lint(&fixture, invocation);
    assert_eq!(report.counts.selected, 3);
    assert!(report.counts.excluded > 0);

    assert!(
        report
            .selection
            .iter()
            .all(|decision| !decision.reason.is_empty())
    );

    let combined = fixture.run(&[
        "lint",
        "selected/one.luau",
        "selected/nested",
        "selected/*.lua",
    ]);

    assert_eq!(combined.status, 0);
    let mut invocation = prepare(&fixture, &["selected/one.luau", "selected/nested"], &[]);

    invocation.invocation.inputs.push(Input::Pattern {
        base: fixture.root().to_path_buf(),
        pattern: "selected/*.lua".to_owned(),
    });

    assert_eq!(lint(&fixture, invocation).counts.selected, 3);
    let explicit = fixture.run(&["lint", "selected/ignored.luau"]);
    assert_eq!(explicit.status, 0);

    assert_eq!(
        lint(&fixture, prepare(&fixture, &["selected/ignored.luau"], &[]))
            .counts
            .selected,
        1
    );

    let excluded = fixture.run(&["lint", "selected/excluded.luau"]);
    assert_eq!(excluded.status, 0);

    let report = lint(
        &fixture,
        prepare(&fixture, &["selected/excluded.luau"], &[]),
    );

    assert_eq!(report.counts.selected, 0);

    assert!(report.selection.iter().any(|decision| !decision.selected
        && decision.input.ends_with("excluded.luau")
        && !decision.reason.is_empty()));

    let overridden = fixture.run(&[
        "lint",
        "selected/excluded.luau",
        "other.luau",
        "--include",
        "**/*.luau",
        "--override-exclusions",
    ]);

    assert_eq!(overridden.status, 0);
    let mut invocation = prepare(&fixture, &["selected/excluded.luau", "other.luau"], &[]);
    invocation.overrides.selection.include = Some(vec!["**/*.luau".to_owned()]);
    invocation.overrides.selection.override_exclusions = true;
    assert_eq!(lint(&fixture, invocation).counts.selected, 2);

    let stdin = fixture.stdin(
        &[
            "lint",
            "-",
            "selected/one.luau",
            "--stdin-filepath",
            "selected/memory.lua",
        ],
        &source,
    );

    assert_eq!(stdin.status, 0);
    assert!(stdin.stderr.contains("memory.lua"));
    let mut invocation = prepare(&fixture, &["selected/one.luau"], &[]);

    invocation.invocation.inputs.push(Input::Stdin {
        text: Arc::from(source.as_str()),
        context: Some(fixture.path("selected/memory.lua")),
    });

    let report = lint(&fixture, invocation);
    assert_eq!(report.counts.selected, 2);

    assert!(report.final_findings.iter().any(|finding| {
        report
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            .is_some_and(|path| path.ends_with("memory.lua"))
    }));

    for input in ["missing.luau", "selected/*.missing"] {
        let output = fixture.run(&["lint", input]);
        assert_eq!(output.status, 2, "{input}: {}", output.stderr);
    }
}

#[test]
fn invalid_rules_settings_and_write_modes_are_rejected_without_mutation() {
    let fixture = Fixture::new("cli", "lint");
    let original = fixture.bytes("fixing.luau");

    let cases: &[&[&str]] = &[
        &["--rule", "nonexistent"],
        &["--preset", "nonexistent"],
        &["--fix-rule", "self_assignment"],
        &["--fix-stdin"],
        &["--fix", "--fix-preview"],
        &["--jobs", "0"],
        &["--include", "["],
        &["--set", "linting.nonexistent=true"],
        &[
            "--set",
            r#"linting.rules.unused_variable={"enabled":true,"severity":"Warning","settings":{"rule":"unused_variable","parameters":{"ignore_pattern":"["}}}"#,
        ],
        &[
            "--set",
            r#"linting.rules.self_assignment={"enabled":true,"severity":"Warning","settings":{"rule":"divide_by_zero","parameters":{}}}"#,
        ],
        &["--diagnostic-category", "nonexistent"],
    ];

    for extra in cases {
        let mut arguments = vec!["lint", "fixing.luau"];
        arguments.extend_from_slice(extra);
        let output = fixture.run(&arguments);
        assert_eq!(output.status, 2, "{extra:?}: {}", output.stderr);
        assert_eq!(fixture.bytes("fixing.luau"), original);
    }

    let collision = fixture.run(&[
        "lint",
        "fixing.luau",
        "--rule",
        "self_assignment",
        "--output",
        "fixing.luau",
    ]);

    assert_eq!(collision.status, 2, "{}", collision.stderr);
    assert_eq!(fixture.bytes("fixing.luau"), original);
}

#[test]
fn reporting_preserves_identity_color_quiet_and_streaming_boundaries() {
    let fixture = Fixture::new("lint", "assignment");
    let output = fixture.run(&["lint", "positive.luau", "--rule", "self_assignment"]);

    let repeated = fixture.run(&[
        "lint",
        "positive.luau",
        "--rule",
        "self_assignment",
        "--jobs",
        "1",
    ]);

    assert_eq!(repeated.status, output.status);
    let project = fixture.project();
    let invocation = prepare(&fixture, &["positive.luau"], &["self_assignment"]);
    let first = complete(project.lint(invocation.clone(), options())).expect("initial lint");
    let second = complete(project.lint(invocation, options())).expect("repeated lint");

    assert_eq!(
        diagnostic(
            &first.final_findings,
            &first.snapshot,
            "self_assignment",
            "positive.luau",
            [17, 22]
        )
        .identity,
        diagnostic(
            &second.final_findings,
            &second.snapshot,
            "self_assignment",
            "positive.luau",
            [17, 22]
        )
        .identity
    );

    complete(project.finish()).expect("worker cleanup");

    for color in ["auto", "never", "always"] {
        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--rule",
            "self_assignment",
            "--color",
            color,
            "--quiet",
        ]);

        assert_eq!(output.status, 0);
        assert!(output.stderr.contains("self_assignment"));
        assert!(output.stderr.contains("positive.luau"));
        assert!(console::strip_ansi_codes(&output.stderr).contains("value=value"));
        assert!(!output.stderr.contains("Linting complete:"));
        assert_eq!(output.stderr.contains("\u{1b}["), color == "always");
    }

    let streaming = fixture.run(&[
        "lint",
        "positive.luau",
        "--streaming",
        "--rule",
        "self_assignment",
    ]);

    assert_eq!(streaming.status, 0);
    assert_eq!(streaming.stdout, "");

    assert_eq!(
        streaming.stderr.matches("warning[self_assignment]").count(),
        1
    );

    assert!(streaming.stderr.contains("positive.luau"));
    assert!(streaming.stderr.contains("value=value"));
    assert!(streaming.stderr.contains("Linting complete:"));
}

#[test]
fn configuration_inheritance_executable_settings_and_schema_are_real() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write("instar.toml", "[linting]\nselected = ['self_assignment']\n");

    fixture.write(
        "child/instar.toml",
        "[linting]\nselected = ['parenthesized_conditions']\n",
    );

    fixture.write("child/input.luau", fixture.bytes("fixing.luau"));

    let mut request = prepare(&fixture, &["fixing.luau", "child/input.luau"], &[]);
    request.invocation.project_root = Some(fixture.path("."));

    let inherited = fixture.run(&[
        "lint",
        "fixing.luau",
        "child/input.luau",
        "--project-root",
        ".",
    ]);

    let inherited_report = lint(&fixture, request);

    assert_eq!(inherited.status, 0);
    let report = inherited_report;
    assert_eq!(report.final_findings.len(), 2);
    let settings = report.settings;
    assert_eq!(settings.len(), 2);

    assert!(settings.iter().any(|effective| {
        effective.settings.linting.rules[&RuleIdentity::SelfAssignment].enabled
            && !effective.settings.linting.rules[&RuleIdentity::ParenthesizedConditions].enabled
    }));

    assert!(settings.iter().any(|effective| {
        effective.settings.linting.rules[&RuleIdentity::ParenthesizedConditions].enabled
            && !effective.settings.linting.rules[&RuleIdentity::SelfAssignment].enabled
    }));

    fixture.write("alternate.toml", "[linting]\npresets = ['none']\n");

    let mut request = prepare(&fixture, &["fixing.luau"], &[]);
    request.invocation.configuration = Some(fixture.path("alternate.toml"));
    let explicit = fixture.run(&["lint", "fixing.luau", "--configuration", "alternate.toml"]);
    let explicit_report = lint(&fixture, request);

    assert_eq!(explicit.status, 0);
    assert_eq!(explicit_report.final_findings, []);
    fixture.write("child/instar.toml", "");

    fixture.write(
        "child/.luaurc",
        r#"{"languageMode":"strict","lint":{"LocalUnused":false},"globals":["external"]}"#,
    );

    fixture.write("child/input.luau", fixture.bytes("globals.luau"));

    let request = prepare(&fixture, &["child/input.luau"], &["UnknownGlobal"]);
    let language = fixture.run(&["lint", "child/input.luau", "--rule", "UnknownGlobal"]);
    let language_report = lint(&fixture, request);

    assert_eq!(language.status, 0);
    assert_eq!(language_report.final_findings, []);

    fixture.write(
        "executable.luau",
        "return { linting = { selected = { 'self_assignment' } } }\n",
    );

    let mut request = prepare(&fixture, &["fixing.luau"], &[]);
    request.invocation.configuration = Some(fixture.path("executable.luau"));
    let executable = fixture.run(&["lint", "fixing.luau", "--configuration", "executable.luau"]);
    let executable_report = lint(&fixture, request);

    assert_eq!(executable.status, 0);

    assert_eq!(executable_report.final_findings.len(), 1);

    let schema = Project::configuration_schema();
    assert!(schema.is_object());
    assert!(schema.to_string().contains("\"linting\""));
    assert!(schema.to_string().contains("mismatched_argument_count"));
    assert!(schema.to_string().contains("ignore_pattern"));
}

#[test]
fn sandbox_and_type_function_resource_limits_never_report_clean_completion() {
    let fixture = Fixture::new("cli", "lint");
    let mut reports = Vec::new();

    for configuration in ["sandbox.luau", "budget.luau"] {
        let mut arguments = vec![
            "lint",
            "fixing.luau",
            "--configuration",
            configuration,
            "--rule",
            "self_assignment",
        ];

        if configuration == "budget.luau" {
            arguments.extend(["--timeout", "1"]);
        }

        let output = fixture.run(&arguments);
        assert_ne!(output.status, 0, "{configuration}");
        let project = fixture.project();
        let mut invocation = prepare(&fixture, &["fixing.luau"], &["self_assignment"]);
        invocation.invocation.configuration = Some(fixture.path(configuration));

        let observed = if configuration == "budget.luau" {
            Options::new(Some(Instant::now() + Duration::from_secs(1)))
        } else {
            options()
        };

        let failure = complete(project.lint(invocation, observed))
            .expect_err("bounded configuration failure");

        assert_ne!(
            failure.completion,
            Completion::Complete,
            "{configuration}: {failure:?}"
        );

        assert!(match &failure.completion {
            Completion::TimedOut | Completion::Cancelled => true,
            Completion::Incomplete(reasons) => !reasons.is_empty(),
            Completion::Complete => false,
        });

        let sandbox = if configuration == "sandbox.luau" {
            assert!(output.stderr.contains(configuration));

            let captured = project.snapshot();

            let finding = diagnostic(
                &failure.diagnostics,
                &captured,
                "configuration",
                configuration,
                [0, fixture.bytes(configuration).len()],
            );

            assert_eq!(finding.category, DiagnosticCategory::Configuration);
            assert_eq!(finding.severity, Severity::Error);
            assert_ne!(finding.message, "");
            assert_eq!(finding.identity.location(), finding.primary);
            assert!(finding.identity.inputs().contains(&finding.primary.source));

            assert!(
                failure
                    .completed_sources
                    .iter()
                    .any(|outcome| outcome.source == finding.primary.source)
            );

            Some(attributed(from_ref(finding), &captured)[0].clone())
        } else {
            None
        };

        reports.push(fixture.normalize(&json!({"case": configuration, "failed": output.status != 0, "incomplete": true, "sandbox": sandbox})));
        complete(project.finish()).expect("worker cleanup");
    }

    let output = fixture.run(&[
        "lint",
        "endless.luau",
        "--rule",
        "mismatched_argument_count",
        "--timeout",
        "1",
    ]);

    assert_ne!(output.status, 0);
    let project = fixture.project();

    let result = complete(project.lint(
        prepare(&fixture, &["endless.luau"], &["mismatched_argument_count"]),
        Options::new(Some(Instant::now() + Duration::from_secs(1))),
    ));

    let completion = match result {
        Ok(report) => report.completion,
        Err(failure) => failure.completion,
    };

    assert_ne!(completion, Completion::Complete);

    assert!(match &completion {
        Completion::TimedOut | Completion::Cancelled => true,
        Completion::Incomplete(reasons) => !reasons.is_empty(),
        Completion::Complete => false,
    });

    reports.push(json!({"case": "functions", "failed": output.status != 0, "incomplete": true}));
    complete(project.finish()).expect("worker cleanup");
    snapshot(&fixture, "lint", "resources", &json!(reports));
}

#[test]
fn dependencies_are_linted_even_when_excluded() {
    let fixture = Fixture::new("cli", "lint");

    let mut request = prepare(
        &fixture,
        &["imported.luau"],
        &["unused_variable", "mismatched_argument_count"],
    );

    request.overrides.selection.exclude = Some(vec!["dependency.luau".to_owned()]);
    request.invocation.explanations.dependencies = true;

    let output = fixture.run(&[
        "lint",
        "imported.luau",
        "--rule",
        "unused_variable",
        "--rule",
        "mismatched_argument_count",
        "--exclude",
        "dependency.luau",
        "--explain-dependencies",
    ]);

    let report = lint(&fixture, request);

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert!(output.stderr.contains("Dependencies:"));
    assert!(output.stderr.contains("Imports:"));
    assert!(output.stderr.contains("warning[unused_variable]"));
    assert!(output.stderr.contains("warning[mismatched_argument_count]"));
    assert!(output.stderr.contains("dependency.luau"));
    assert!(output.stderr.contains("imported.luau"));

    let dependency = fixture.read("dependency.luau");
    let start = dependency.find("unused").expect("dependency binding");

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "unused_variable",
        "dependency.luau",
        [start, start + 6],
    );

    let source = fixture.read("imported.luau");
    let start = source.find("dependency()").expect("resolved call");

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "mismatched_argument_count",
        "imported.luau",
        [start, start + 12],
    );

    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.dependencies, 1);
    assert_eq!(report.completion, Completion::Complete);
}

#[test]
fn import_restrictions_and_shadowed_builtins_use_actual_bindings() {
    let fixture = Fixture::new("cli", "lint");
    let setting = r#"linting.rules.restricted_module_paths={"enabled":true,"severity":"Warning","settings":{"rule":"restricted_module_paths","parameters":{"paths":{"./dependency":"use the public module"}}}}"#;

    let mut request = prepare(
        &fixture,
        &["restricted.luau"],
        &["non_const_require", "restricted_module_paths"],
    );

    request.overrides.linting.rules = Some(BTreeMap::from([(
        RuleIdentity::named("restricted_module_paths").expect("known rule"),
        serde_json::from_str(setting.split_once('=').expect("setting value").1)
            .expect("typed rule configuration"),
    )]));

    let output = fixture.run(&[
        "lint",
        "restricted.luau",
        "--rule",
        "non_const_require",
        "--rule",
        "restricted_module_paths",
        "--set",
        setting,
    ]);

    let report = lint(&fixture, request);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("restricted_module_paths"));

    let source = fixture.read("restricted.luau");
    let start = source.find("path").expect("dynamic require argument");

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "non_const_require",
        "restricted.luau",
        [start, start + 4],
    );

    let text = "'./dependency'";
    let start = source.find(text).expect("restricted require argument");

    let finding = diagnostic(
        &report.final_findings,
        &report.snapshot,
        "restricted_module_paths",
        "restricted.luau",
        [start, start + text.len()],
    );

    assert_eq!(
        finding.message,
        "module path ./dependency is restricted: use the public module"
    );

    let fixture = Fixture::new("lint", "shadowing");

    for (name, rule) in [
        ("protection.luau", "ignored_pcall_result"),
        ("function.luau", "mismatched_argument_count"),
    ] {
        let original = fixture.bytes(name);
        let request = prepare(&fixture, &[name], &[rule]);
        let output = fixture.run(&["lint", name, "--rule", rule]);
        let report = lint(&fixture, request);
        assert_eq!(output.stdout, "");

        assert!(
            report
                .final_findings
                .iter()
                .all(|finding| finding.code != rule)
        );

        assert_eq!(fixture.bytes(name), original);
    }
}

#[test]
fn source_pragmas_disable_type_diagnostics_without_disabling_semantic_rules() {
    let fixture = Fixture::new("cli", "lint");

    let request = prepare(&fixture, &["pragmas.luau"], &["implicit_any_parameter"]);
    let output = fixture.run(&["lint", "pragmas.luau", "--rule", "implicit_any_parameter"]);
    let report = lint(&fixture, request);

    assert_eq!(output.status, 0, "{}", output.stderr);

    let source = fixture.read("pragmas.luau");

    let start = source
        .find("parameter")
        .expect("contextually inferred any parameter");

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "implicit_any_parameter",
        "pragmas.luau",
        [start, start + 9],
    );

    assert_eq!(report.final_findings.len(), 1);
    assert_eq!(report.completion, Completion::Complete);
}

#[test]
fn roblox_pairs_use_offline_assets_and_preserve_local_shadowing() {
    let assets = Fixture::new("project", "assets/project");
    let collection = Fixture::empty();
    let mut reports = Vec::new();

    for (scenario, name, rule, range) in [
        (
            "color",
            "positive.luau",
            "roblox_incorrect_color3_new_bounds",
            Some([10, 29]),
        ),
        (
            "scale",
            "positive.luau",
            "roblox_manual_fromscale_or_fromoffset",
            Some([10, 32]),
        ),
        (
            "players",
            "positive.luau",
            "roblox_prefer_get_players",
            Some([51, 72]),
        ),
        (
            "dimensions",
            "positive.luau",
            "roblox_suspicious_udim2_new",
            Some([10, 24]),
        ),
        (
            "shadowing",
            "color.luau",
            "roblox_incorrect_color3_new_bounds",
            None,
        ),
    ] {
        let fixture = Fixture::new("lint", scenario);

        for name in [
            "metadata.json",
            "roblox.d.luau",
            "local.d.luau",
            "plugin.d.luau",
            "none.d.luau",
            "enumerations.d.luau",
            "documentation.json",
        ] {
            fixture.write(&format!("assets/{name}"), assets.bytes(name));
        }

        let directory = format!(
            "environment.asset_directory={}",
            json!(fixture.path("assets"))
        );

        let mut request = prepare(&fixture, &[name], &[rule]);
        request.overrides.environment.roblox = Some(true);
        request.overrides.environment.asset_directory = Some(fixture.path("assets"));

        let output = fixture.run(&[
            "lint",
            name,
            "--rule",
            rule,
            "--set",
            "environment.roblox=true",
            "--set",
            &directory,
        ]);

        let report = lint(&fixture, request);

        assert_eq!(output.status, 0, "{}", output.stderr);

        assert_eq!(report.completion, Completion::Complete);

        if let Some(range) = range {
            diagnostic(&report.final_findings, &report.snapshot, rule, name, range);

            assert_eq!(report.final_findings.len(), 1);
        } else {
            assert_eq!(report.final_findings, []);
        }

        reports.push(fixture.normalize(&json!({
            "scenario": scenario, "rule": rule, "findings": report.final_findings,
            "sources": report.sources, "availability": report.availability,
            "counts": report.counts, "completion": report.completion,
        })));
    }

    snapshot(&collection, "lint", "roblox", &json!(reports));
}

#[test]
fn public_cancellation_and_deadline_outcomes_are_not_clean_results() {
    let fixture = Fixture::new("cli", "lint");
    let original = fixture.bytes("fixing.luau");
    let project = fixture.project();

    let request = LintRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(fixture.path("fixing.luau"))],
            ..Invocation::default()
        },
        overrides: LintOverrides {
            linting: LintPatch {
                selected: Some(BTreeSet::from([RuleIdentity::SelfAssignment])),
                ..LintPatch::default()
            },
            ..LintOverrides::default()
        },
        ..LintRequest::default()
    };

    let cancelled = options();
    cancelled.cancellation.cancel();

    let failure =
        complete(project.lint(request.clone(), cancelled)).expect_err("cancelled invocation");

    assert_eq!(failure.completion, Completion::Cancelled);
    let expired = Options::new(Some(Instant::now()));
    let failure = complete(project.lint(request, expired)).expect_err("expired total budget");
    assert_eq!(failure.completion, Completion::TimedOut);
    assert_eq!(fixture.bytes("fixing.luau"), original);
    complete(project.finish()).expect("worker cleanup");
}

#[test]
fn unchanged_inputs_reuse_identities_and_environment_changes_invalidate_inference() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write("instar.toml", "[linting]\nselected = ['implicit_any_local']\n[[environment.groups]]\nnamespace = '@project'\ndefinitions = ['declaration.luau']\n");
    let project = fixture.project();

    let request = LintRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(fixture.path("globals.luau"))],
            ..Invocation::default()
        },
        ..LintRequest::default()
    };

    let first = complete(project.lint(request.clone(), options())).expect("initial inference");
    let repeated = complete(project.lint(request.clone(), options())).expect("unchanged inference");
    assert_eq!(first.completion, Completion::Complete);
    assert_eq!(first.final_findings.len(), 1);
    assert_eq!(first.final_findings, repeated.final_findings);
    assert_eq!(first.sources, repeated.sources);
    assert_eq!(first.inputs, repeated.inputs);
    assert!(project.current(&repeated.inputs));
    fixture.write("declaration.luau", fixture.bytes("numbers.luau"));

    let changed =
        complete(project.lint(request, options())).expect("changed environment inference");

    assert_eq!(changed.completion, Completion::Complete);
    assert_eq!(changed.final_findings, []);
    assert_ne!(first.inputs, changed.inputs);
    assert!(!project.current(&first.inputs));
    assert!(project.current(&changed.inputs));
    complete(project.finish()).expect("worker cleanup");
}

#[test]
fn watch_relints_changed_source_rules_and_declaration_inputs() {
    let fixture = Fixture::new("cli", "lint");
    fixture.write("instar.toml", "[linting]\nselected = ['implicit_any_local']\n[[environment.groups]]\nnamespace = '@project'\ndefinitions = ['declaration.luau']\n");
    let project = fixture.project();

    let request = WatchRequest {
        operation: WatchOperation::Lint(LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(fixture.path("globals.luau"))],
                ..Invocation::default()
            },
            ..LintRequest::default()
        }),
        poll_interval: Duration::from_millis(10),
    };

    let first =
        complete(project.watch(request.clone(), options())).expect("initial watch iteration");

    assert_eq!(first.completion, Completion::Complete);
    assert_eq!(first.iterations.len(), 1);

    let WatchCommandReport::Lint(initial) = &first.iterations[0].report else {
        panic!("watch must return lint report");
    };

    assert_eq!(initial.final_findings.len(), 1);
    fixture.write("globals.luau", fixture.bytes("pragmas.luau"));

    let source =
        complete(project.watch(request.clone(), options())).expect("source-triggered watch");

    assert_eq!(source.iterations.len(), 1);
    assert_ne!(source.iterations[0].changed, []);

    let WatchCommandReport::Lint(changed) = &source.iterations[0].report else {
        panic!("watch must return lint report");
    };

    assert_eq!(changed.final_findings, []);
    fixture.write("instar.toml", "[linting]\nselected = ['implicit_any_parameter']\n[[environment.groups]]\nnamespace = '@project'\ndefinitions = ['declaration.luau']\n");

    let rules =
        complete(project.watch(request.clone(), options())).expect("configuration-triggered watch");

    assert_eq!(rules.iterations.len(), 1);
    assert_ne!(rules.iterations[0].changed, []);

    let WatchCommandReport::Lint(changed) = &rules.iterations[0].report else {
        panic!("watch must return lint report");
    };

    assert_eq!(changed.final_findings.len(), 1);
    assert_eq!(changed.final_findings[0].code, "implicit_any_parameter");
    fixture.write("declaration.luau", fixture.bytes("invalid.luau"));

    let environment =
        complete(project.watch(request, options())).expect("environment-triggered watch");

    assert_ne!(environment.completion, Completion::Complete);
    assert_eq!(environment.iterations.len(), 1);
    assert_ne!(environment.iterations[0].changed, []);
    complete(project.finish()).expect("worker cleanup");
}

#[test]
fn public_events_separate_progress_and_findings_with_original_source_identity() {
    let fixture = Fixture::new("lint", "division");
    let project = fixture.project();
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut observed = options();
    let collected = Arc::clone(&events);

    observed.observer = Some(Arc::new(move |event| {
        collected.lock().expect("event collection").push(event);
    }));

    let request = LintRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(fixture.path("positive.luau"))],
            ..Invocation::default()
        },
        overrides: LintOverrides {
            linting: LintPatch {
                selected: Some(BTreeSet::from([RuleIdentity::DivideByZero])),
                ..LintPatch::default()
            },
            ..LintOverrides::default()
        },
        ..LintRequest::default()
    };

    let report = complete(project.lint(request, observed)).expect("observed lint");
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.final_findings.len(), 1);

    {
        let events = events.lock().expect("event collection");

        assert!(
            events
                .iter()
                .any(|event| matches!(event.payload, EventPayload::Progress(_)))
        );

        assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::Diagnostic(finding) if finding.identity == report.final_findings[0].identity)));
        assert!(events.iter().any(|event| matches!(&event.payload, EventPayload::SourceOutcome(outcome) if report.sources.contains(&outcome.source))));
    }

    complete(project.finish()).expect("worker cleanup");
}

#[test]
fn validated_fixes_reject_external_changes_and_identify_write_failures() {
    for directory in [false, true] {
        let inputs = Fixture::new("cli", "lint");
        let fixture = Fixture::empty();

        for name in ["instar.toml", "fixing.luau", "fixed.luau"] {
            fixture.write(name, inputs.bytes(name));
        }

        let project = fixture.project();

        let request = LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(fixture.path("fixing.luau"))],
                ..Invocation::default()
            },
            overrides: LintOverrides {
                linting: LintPatch {
                    selected: Some(BTreeSet::from([RuleIdentity::SelfAssignment])),
                    ..LintPatch::default()
                },
                ..LintOverrides::default()
            },
            fixes: FixMode::Preview,
            ..LintRequest::default()
        };

        let report = complete(project.lint(request, options())).expect("validated fix preview");
        assert_eq!(report.completion, Completion::Complete);
        assert_eq!(report.proposals.len(), 1);
        let edit = report.proposals[0].clone();
        let source = edit.proposal().guards[0];

        if directory {
            fs::remove_file(fixture.path("fixing.luau")).expect("remove private fixture source");
            fs::create_dir(fixture.path("fixing.luau")).expect("nonregular write destination");
        } else {
            fixture.write("fixing.luau", fixture.bytes("fixed.luau"));
        }

        let applied = complete(project.apply(
            ApplyRequest {
                edit,
                destinations: vec![WriteAuthorization {
                    source,
                    destination: fixture.path("fixing.luau"),
                }],
            },
            options(),
        ))
        .expect("individual failed write outcome");

        assert_ne!(applied.completion, Completion::Complete);
        assert_eq!(applied.writes.len(), 1);

        let WriteOutcome::Failed {
            source: failed,
            message,
        } = &applied.writes[0]
        else {
            panic!("write failure cannot be a successful or unapplied fix");
        };

        assert_eq!(*failed, source);
        assert_ne!(message.as_str(), "");

        if directory {
            assert!(fixture.path("fixing.luau").is_dir());
        } else {
            assert_eq!(fixture.read("fixing.luau"), fixture.read("fixed.luau"));
        }

        complete(project.finish()).expect("worker cleanup");
    }
}

#[test]
fn shared_project_exports_and_ordered_declarations_supply_real_signatures() {
    let fixture = Fixture::new("cli", "project");
    let inputs = Fixture::new("cli", "lint");
    fixture.write("shared.luau", inputs.bytes("shared.luau"));
    fixture.write("declaration.luau", inputs.bytes("numbers.luau"));
    fixture.write("instar.toml", format!("{}\n[[environment.groups]]\nnamespace = '@additional'\ndefinitions = ['declaration.luau']\n", fixture.read("instar.toml")));

    let mut request = prepare(&fixture, &["shared.luau"], &["mismatched_argument_count"]);
    request.invocation.explanations.dependencies = true;

    let output = fixture.run(&[
        "lint",
        "shared.luau",
        "--rule",
        "mismatched_argument_count",
        "--explain-dependencies",
    ]);

    let report = lint(&fixture, request);

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert!(output.stderr.contains("Dependencies:"));
    assert!(output.stderr.contains("Imports:"));

    assert_eq!(report.completion, Completion::Complete);
    let source = fixture.read("shared.luau");

    for call in ["input.count(1)", "library.double(1, 2)", "documented()"] {
        let start = source.find(call).expect("shared resolved call");

        diagnostic(
            &report.final_findings,
            &report.snapshot,
            "mismatched_argument_count",
            "shared.luau",
            [start, start + call.len()],
        );
    }

    assert_eq!(report.final_findings.len(), 3);

    assert_eq!(report.counts.dependencies, 2);

    for settings in report.settings {
        let groups = settings.settings.environment.groups;

        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].namespace, "@contract");
        assert_eq!(groups[1].namespace, "@additional");
    }
}

#[test]
fn mapped_instances_resolve_distinct_dependencies_without_duplicating_findings() {
    let fixture = Fixture::new("cli", "mapping");
    let assets = Fixture::new("project", "assets/project");

    for name in [
        "metadata.json",
        "roblox.d.luau",
        "local.d.luau",
        "plugin.d.luau",
        "none.d.luau",
        "enumerations.d.luau",
        "documentation.json",
    ] {
        fixture.write(&format!("assets/{name}"), assets.bytes(name));
    }

    let directory = format!(
        "environment.asset_directory={}",
        json!(fixture.path("assets"))
    );

    let mut request = prepare(&fixture, &["shared.luau"], &["divide_by_zero"]);
    request.invocation.explanations.dependencies = true;
    request.overrides.environment.asset_directory = Some(fixture.path("assets"));

    let output = fixture.run(&[
        "lint",
        "shared.luau",
        "--rule",
        "divide_by_zero",
        "--explain-dependencies",
        "--set",
        &directory,
    ]);

    let report = lint(&fixture, request);

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert!(output.stderr.contains("Dependencies:"));
    assert!(output.stderr.contains("Imports:"));

    assert_eq!(report.completion, Completion::Complete);

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "divide_by_zero",
        "shared.luau",
        [40, 41],
    );

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "divide_by_zero",
        "first.luau",
        [9, 10],
    );

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "divide_by_zero",
        "second.luau",
        [9, 10],
    );

    assert_eq!(report.final_findings.len(), 3);

    assert_eq!(report.modules.len(), 4);

    assert_eq!(report.sources.len(), 3);

    assert_eq!(report.dependencies.len(), 2);
}

#[test]
fn related_shadow_locations_explain_the_original_declaration() {
    let fixture = Fixture::new("cli", "lint");
    let source = fixture.read("positive.luau");

    let original = source
        .match_indices("shadow")
        .next()
        .expect("original declaration")
        .0;

    let shadow = source
        .match_indices("shadow")
        .nth(1)
        .expect("shadow declaration")
        .0;

    let request = prepare(&fixture, &["positive.luau"], &["LocalShadow"]);
    let output = fixture.run(&["lint", "positive.luau", "--rule", "LocalShadow"]);
    let report = lint(&fixture, request);
    assert_eq!(output.status, 0, "{}", output.stderr);

    let finding = diagnostic(
        &report.final_findings,
        &report.snapshot,
        "LocalShadow",
        "positive.luau",
        [shadow, shadow + 6],
    );

    assert!(finding.related.iter().any(|related| {
        [related.location.span.start, related.location.span.end] == [original, original + 6]
            && related.location.source == finding.primary.source
            && !related.message.is_empty()
    }));
}

#[test]
fn operational_logging_preserves_human_diagnostics() {
    let fixture = Fixture::new("lint", "division");

    let output = fixture.run(&[
        "lint",
        "positive.luau",
        "--rule",
        "divide_by_zero",
        "--verbose",
    ]);

    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("divide_by_zero"));
    assert!(output.stderr.contains("positive.luau"));
    assert!(output.stderr.contains("DEBUG"));

    let report = lint(
        &fixture,
        prepare(&fixture, &["positive.luau"], &["divide_by_zero"]),
    );

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "divide_by_zero",
        "positive.luau",
        [12, 16],
    );
}

#[test]
fn immutable_binding_exemptions_respect_actual_shadowed_mutation() {
    let fixture = Fixture::new("lint", "shadowing");
    let setting = r#"linting.rules.prefer_const={"enabled":true,"severity":"Warning","settings":{"rule":"prefer_const","parameters":{"mutated_tables_stay_local":true}}}"#;

    let mut request = prepare(&fixture, &["table.luau"], &["prefer_const"]);

    request.overrides.linting.rules = Some(BTreeMap::from([(
        RuleIdentity::named("prefer_const").expect("known rule"),
        serde_json::from_str(setting.split_once('=').expect("setting value").1)
            .expect("typed rule configuration"),
    )]));

    let shadow = fixture.run(&[
        "lint",
        "table.luau",
        "--rule",
        "prefer_const",
        "--set",
        setting,
    ]);

    let report = lint(&fixture, request);

    assert_eq!(shadow.status, 0);

    diagnostic(
        &report.final_findings,
        &report.snapshot,
        "prefer_const",
        "table.luau",
        [6, 7],
    );

    let mut request = prepare(&fixture, &["mutation.luau"], &["prefer_const"]);

    request.overrides.linting.rules = Some(BTreeMap::from([(
        RuleIdentity::named("prefer_const").expect("known rule"),
        serde_json::from_str(setting.split_once('=').expect("setting value").1)
            .expect("typed rule configuration"),
    )]));

    let mutation = fixture.run(&[
        "lint",
        "mutation.luau",
        "--rule",
        "prefer_const",
        "--set",
        setting,
    ]);

    let mutation_report = lint(&fixture, request);

    assert_eq!(mutation.status, 0);
    assert_eq!(mutation_report.final_findings, []);
    let fixture = Fixture::new("lint", "escapes");
    let request = prepare(&fixture, &["byte.luau"], &["bad_string_escape"]);
    let bytes = fixture.run(&["lint", "byte.luau", "--rule", "bad_string_escape"]);
    let bytes_report = lint(&fixture, request);
    assert_eq!(bytes.status, 0);
    assert_eq!(bytes_report.final_findings, []);
}

#[test]
fn declared_globals_and_warning_modes_follow_source_policy() {
    let fixture = Fixture::new("cli", "lint");
    let source = fixture.read("positive.luau");

    for (mode, unknown, implicit) in [("strict", 0, 0), ("nonstrict", 0, 1), ("nocheck", 1, 1)] {
        fixture.write(".luaurc", json!({"languageMode": mode}).to_string());

        let request = prepare(
            &fixture,
            &["positive.luau"],
            &["UnknownGlobal", "ImplicitReturn"],
        );

        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--rule",
            "UnknownGlobal",
            "--rule",
            "ImplicitReturn",
        ]);

        let report = lint(&fixture, request);

        assert_eq!(output.status, 0, "{mode}: {}", output.stdout);

        assert_eq!(report.completion, Completion::Complete);

        assert_eq!(report.final_findings.len(), unknown + implicit);

        let count = report
            .final_findings
            .iter()
            .filter(|finding| finding.code == "UnknownGlobal")
            .count();

        assert_eq!(count, unknown, "{mode}");

        if unknown > 0 {
            let start = source.find("missing").expect("unknown reference");

            diagnostic(
                &report.final_findings,
                &report.snapshot,
                "UnknownGlobal",
                "positive.luau",
                [start, start + 7],
            );
        }

        if implicit > 0 {
            let text = "if flag then return 1 end";
            let start = source.find(text).expect("fallthrough branch");

            diagnostic(
                &report.final_findings,
                &report.snapshot,
                "ImplicitReturn",
                "positive.luau",
                [start, start + text.len()],
            );
        }
    }

    fixture.write(".luaurc", r#"{"languageMode":"nocheck"}"#);

    let mut request = prepare(&fixture, &["positive.luau"], &["UnknownGlobal"]);
    request.overrides.environment.globals = Some(vec!["missing".to_owned()]);

    let output = fixture.run(&[
        "lint",
        "positive.luau",
        "--rule",
        "UnknownGlobal",
        "--set",
        r#"environment.globals=["missing"]"#,
    ]);

    let output_report = lint(&fixture, request);

    assert_eq!(output.status, 0);
    assert_eq!(output_report.final_findings, []);

    let request = prepare(&fixture, &["negative.luau"], &["DeprecatedGlobal"]);
    let deprecated = fixture.run(&["lint", "negative.luau", "--rule", "DeprecatedGlobal"]);
    let deprecated_report = lint(&fixture, request);

    assert_eq!(deprecated.status, 0);
    assert_eq!(deprecated_report.completion, Completion::Complete);
    assert_eq!(deprecated_report.final_findings, []);
}

fn lint(fixture: &Fixture, request: LintRequest) -> LintReport {
    let project = fixture.project();
    let report = complete(project.lint(request, options())).expect("typed lint report");
    complete(project.finish()).expect("worker cleanup");

    report
}

fn prepare(fixture: &Fixture, paths: &[&str], rules: &[&str]) -> LintRequest {
    LintRequest {
        invocation: Invocation {
            inputs: paths
                .iter()
                .map(|path| Input::Path(fixture.path(path)))
                .collect(),
            ..Invocation::default()
        },
        overrides: LintOverrides {
            linting: LintPatch {
                selected: (!rules.is_empty()).then(|| {
                    rules
                        .iter()
                        .map(|rule| RuleIdentity::named(rule).expect("known rule"))
                        .collect()
                }),
                ..LintPatch::default()
            },
            ..LintOverrides::default()
        },
        ..LintRequest::default()
    }
}
