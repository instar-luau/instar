use std::{
    collections::BTreeSet,
    fs,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use instar_lint::RuleIdentity;
use instar_project::{
    ApplyRequest, FixMode, Input, Invocation, LintOverrides, LintPatch, LintRequest, Project,
    WatchCommandReport, WatchOperation, WatchRequest, WriteAuthorization, WriteOutcome,
};
use instar_shared::{Completion, EventPayload, Options};
use serde_json::{Value, json};

use super::support::{Fixture, complete, diagnostic, options, snapshot};

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
        let mut arguments = vec!["lint", "positive.luau", "--json", "--rule", rule];
        if scenario == "deprecation" {
            arguments.extend([
                "--set",
                r#"linting.rules.deprecated={"enabled":true,"severity":"Warning","settings":{"rule":"deprecated","parameters":{"paths":{"old.api":"new.api"}}}}"#,
            ]);
        }
        if scenario == "restrictions" {
            arguments.extend([
                "--set",
                r#"linting.rules.restricted_globals={"enabled":true,"severity":"Warning","settings":{"rule":"restricted_globals","parameters":{"names":{"print":"use logging"}}}}"#,
            ]);
        }
        let original = fixture.bytes("positive.luau");
        let output = fixture.run(&arguments);
        assert_eq!(output.status, 0, "{scenario}: {}", output.stderr);
        let report = output.json();
        assert_eq!(report["completion"], "Complete", "{scenario}: {report}");
        let finding = diagnostic(&report, rule, "positive.luau", range);
        assert_eq!(finding["severity"], "warning");
        assert_eq!(finding["category"], "Lint");
        let findings: Vec<_> = report["diagnostics"]
            .as_array()
            .expect("diagnostic array")
            .iter()
            .filter(|finding| finding["code"] == rule)
            .collect();
        assert_eq!(
            findings.len(),
            1,
            "negative case must not diagnose: {scenario}"
        );
        assert_eq!(fixture.bytes("positive.luau"), original);
        if scenario == "arguments" || scenario == "escapes" || scenario == "invariance" {
            let negative = fixture.run(&["lint", "negative.luau", "--json", "--rule", rule]);
            assert_eq!(negative.status, 0, "{scenario}: {}", negative.stderr);
            assert_eq!(negative.json()["diagnostics"], json!([]));
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
        let output = fixture.run(&["lint", "positive.luau", "--json", "--rule", rule]);
        assert_eq!(output.status, 0, "{rule}: {}", output.stderr);
        let report = output.json();
        assert_eq!(report["completion"], "Complete", "{rule}: {report}");
        let finding = diagnostic(&report, rule, "positive.luau", [start, start + length]);
        assert_eq!(finding["severity"], "warning");
        assert_eq!(finding["category"], "Lint");
        assert!(!finding["message"].as_str().expect("explanation").is_empty());
        let negative = fixture.run(&["lint", "negative.luau", "--json", "--rule", rule]);
        assert_eq!(negative.status, 0, "{rule}: {}", negative.stderr);
        assert_eq!(negative.json()["diagnostics"], json!([]), "{rule}");
        reports.push(fixture.normalize(&json!({
            "rule": rule, "diagnostics": report["diagnostics"], "sources": report["sources"],
            "availability": report["availability"], "counts": report["counts"],
            "completion": report["completion"], "status": report["status"],
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
        let output = fixture.run(&["lint", name, "--json", "--rule", rule]);
        assert_eq!(output.status, 0, "{}", output.stderr);
        let report = output.json();
        diagnostic(&report, rule, name, range);
        let negative = fixture.run(&["lint", clean, "--json", "--rule", rule]);
        assert_eq!(negative.status, 0, "{}", negative.stderr);
        assert_eq!(negative.json()["diagnostics"], json!([]));
        reports.push(fixture.normalize(&json!({
            "rule": rule, "diagnostics": report["diagnostics"], "sources": report["sources"],
            "availability": report["availability"], "counts": report["counts"],
            "completion": report["completion"], "status": report["status"],
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
        let output = fixture.run(&[
            "lint",
            "calls.luau",
            "--json",
            "--rule",
            "mismatched_argument_count",
            "--exclude",
            "module.luau",
        ]);
        assert_eq!(output.status, 0, "{mode}: {}", output.stderr);
        let report = output.json();
        assert_eq!(report["completion"], "Complete", "{mode}: {report}");
        for call in [
            "required(1)",
            "required(1, 2, 3)",
            "optional()",
            "module:method()",
            "module.optional(1, 2, 3)",
        ] {
            let start = source.find(call).expect("literal call");
            diagnostic(
                &report,
                "mismatched_argument_count",
                "calls.luau",
                [start, start + call.len()],
            );
        }
        assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 5);
        assert_eq!(report["counts"]["selected"], 1);
        assert_eq!(report["counts"]["dependencies"], 1);
        assert!(
            report["diagnostics"]
                .as_array()
                .expect("findings")
                .iter()
                .all(|finding| finding["category"] != "Type")
        );
        let valid = fixture.run(&[
            "lint",
            "valid.luau",
            "--json",
            "--rule",
            "mismatched_argument_count",
        ]);
        assert_eq!(valid.status, 0, "{mode}: {}", valid.stderr);
        assert_eq!(valid.json()["completion"], "Complete");
        assert_eq!(valid.json()["diagnostics"], json!([]));
        reports.push(fixture.normalize(&json!({
            "mode": mode, "diagnostics": report["diagnostics"], "sources": report["sources"],
            "availability": report["availability"], "counts": report["counts"],
            "completion": report["completion"], "status": report["status"],
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
        let output = fixture.run(&[
            "lint",
            "globals.luau",
            "--json",
            "--rule",
            "implicit_any_local",
            "--rule",
            "implicit_any_parameter",
        ]);
        assert_eq!(output.status, 0, "{mode}: {}", output.stderr);
        let report = output.json();
        assert_eq!(report["completion"], "Complete", "{mode}: {report}");
        for (rule, text) in [
            ("implicit_any_local", "inferred"),
            ("implicit_any_parameter", "parameter"),
        ] {
            let start = source.find(text).expect("literal binding");
            diagnostic(&report, rule, "globals.luau", [start, start + text.len()]);
        }
        assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 2);
        reports.push(fixture.normalize(&json!({
            "mode": mode, "diagnostics": report["diagnostics"], "sources": report["sources"],
            "availability": report["availability"], "counts": report["counts"],
            "completion": report["completion"], "status": report["status"],
        })));
    }
    let output = fixture.run(&[
        "lint",
        "function.luau",
        "--json",
        "--rule",
        "mismatched_argument_count",
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    let source = fixture.read("function.luau");
    let start = source.find("consume()").expect("typed call");
    diagnostic(
        &report,
        "mismatched_argument_count",
        "function.luau",
        [start, start + "consume()".len()],
    );
    assert_eq!(report["completion"], "Complete");
    reports.push(fixture.normalize(&json!({
        "case": "functions", "diagnostics": report["diagnostics"], "sources": report["sources"],
        "availability": report["availability"], "counts": report["counts"],
        "completion": report["completion"], "status": report["status"],
    })));
    snapshot(&fixture, "lint", "inference", &json!(reports));
}

#[test]
fn recovery_and_missing_prerequisites_retain_useful_findings() {
    let fixture = Fixture::new("cli", "lint");
    let output = fixture.run(&[
        "lint",
        "recovered.luau",
        "--json",
        "--rule",
        "divide_by_zero",
    ]);
    assert_ne!(output.status, 0);
    let report = output.json();
    diagnostic(&report, "divide_by_zero", "recovered.luau", [19, 20]);
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["category"] == "Syntax")
    );
    assert!(fixture.read("recovered.luau").contains("local ="));

    fixture.write("instar.toml", "[linting]\npresets = ['none']\n[[environment.groups]]\nnamespace = '@broken'\ndefinitions = ['invalid.luau']\n");
    let output = fixture.run(&[
        "lint",
        "globals.luau",
        "--json",
        "--rule",
        "implicit_any_local",
        "--rule",
        "divide_by_zero",
    ]);
    assert_ne!(output.status, 0);
    let report = output.json();
    assert_ne!(report["completion"], "Complete");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("invalid.luau")))
    );
    assert!(
        !report["availability"]
            .as_array()
            .expect("coverage")
            .is_empty()
    );
}

#[test]
fn suppression_scopes_and_mistakes_have_precise_locations() {
    let fixture = Fixture::new("cli", "lint");
    let output = fixture.run(&[
        "lint",
        "suppression.luau",
        "--json",
        "--rule",
        "divide_by_zero",
    ]);
    assert_eq!(output.status, 1, "{}", output.stderr);
    let report = output.json();
    let source = fixture.read("suppression.luau");
    for expression in ["3 / 0", "4 / 0"] {
        let start = source.find(expression).expect("visible expression") + 4;
        diagnostic(
            &report,
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
        let finding = diagnostic(&report, "lint.directive", "suppression.luau", [start, end]);
        assert_eq!(finding["message"], message);
    }
    assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 4);
    let output = fixture.run(&["lint", "file.luau", "--json", "--rule", "divide_by_zero"]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.json()["diagnostics"], json!([]));
}

#[test]
fn rule_policy_presets_thresholds_and_filters_remain_independent() {
    let fixture = Fixture::new("lint", "bindings");
    let disabled = fixture.run(&["lint", "positive.luau", "--json", "--preset", "none"]);
    assert_eq!(disabled.status, 0);
    assert_eq!(disabled.json()["diagnostics"], json!([]));
    for (severity, threshold, status) in [
        ("Information", "information", 1),
        ("Warning", "error", 0),
        ("Error", "error", 1),
    ] {
        let setting = format!(
            r#"linting.rules.unused_variable={{"enabled":true,"severity":"{severity}","settings":{{"rule":"unused_variable","parameters":{{}}}}}}"#
        );
        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--json",
            "--rule",
            "unused_variable",
            "--set",
            &setting,
            "--failure-threshold",
            threshold,
        ]);
        assert_eq!(output.status, status, "{}", output.stderr);
        let report = output.json();
        let finding = diagnostic(&report, "unused_variable", "positive.luau", [9, 15]);
        assert_eq!(
            finding["severity"],
            if severity == "Information" {
                "info".to_owned()
            } else {
                severity.to_lowercase()
            }
        );
    }
    for filter in [
        ["--diagnostic-code", "must_use"],
        ["--diagnostic-category", "syntax"],
        ["--diagnostic-severity", "error"],
    ] {
        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--json",
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
        let report = output.json();
        assert_eq!(report["diagnostics"], json!([]));
        assert_eq!(report["failed"], true);
        assert_eq!(report["completion"], "Complete");
    }
    let duplicate = fixture.run(&[
        "lint",
        "positive.luau",
        "--json",
        "--rule",
        "unused_variable",
        "--rule",
        "LocalUnused",
    ]);
    assert_eq!(duplicate.status, 0);
    assert_eq!(
        duplicate.json()["diagnostics"]
            .as_array()
            .expect("deduplicated findings")
            .len(),
        1
    );

    let fixture = Fixture::new("lint", "shadowing");
    let setting = r#"linting.rules.unused_variable={"enabled":true,"severity":"Warning","settings":{"rule":"unused_variable","parameters":{"parameters":true,"loop_variables":true,"ignore_pattern":"^skip$"}}}"#;
    let output = fixture.run(&[
        "lint",
        "input.luau",
        "--json",
        "--rule",
        "unused_variable",
        "--set",
        setting,
    ]);
    assert_eq!(output.status, 0);
    let report = output.json();
    let source = fixture.read("input.luau");
    for text in ["value", "kept"] {
        let start = source.find(text).expect("unread declaration");
        diagnostic(
            &report,
            "unused_variable",
            "input.luau",
            [start, start + text.len()],
        );
    }
    assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 2);

    let fixture = Fixture::new("lint", "constants");
    for (preset, expected) in [
        ("none", 0),
        ("correctness", 0),
        ("recommended", 1),
        ("all", 1),
    ] {
        let output = fixture.run(&["lint", "positive.luau", "--json", "--preset", preset]);
        assert_eq!(output.status, 0, "{preset}: {}", output.stderr);
        let report = output.json();
        let count = report["diagnostics"]
            .as_array()
            .expect("preset findings")
            .iter()
            .filter(|finding| finding["code"] == "prefer_const")
            .count();
        assert_eq!(count, expected, "{preset}");
    }
    let disabled = fixture.run(&[
        "lint", "positive.luau", "--json", "--preset", "all", "--set",
        r#"linting.rules.prefer_const={"enabled":false,"severity":"Warning","settings":{"rule":"prefer_const","parameters":{}}}"#,
    ]);
    assert_eq!(disabled.status, 0);
    assert!(
        disabled.json()["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .all(|finding| finding["code"] != "prefer_const")
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
        "--json",
        "--rule",
        "self_assignment",
        "--rule",
        "parenthesized_conditions",
        "--warnings-as-errors",
    ];
    let analysis = fixture.run(&arguments);
    assert_eq!(analysis.status, 1);
    assert_eq!(fixture.bytes("fixing.luau"), original);
    assert_eq!(
        analysis.json()["fixes"]
            .as_array()
            .expect("suggested fixes")
            .len(),
        2
    );
    assert_eq!(analysis.json()["writes"], json!([]));
    let mut preview_arguments = arguments.to_vec();
    preview_arguments.push("--fix-preview");
    let preview = fixture.run(&preview_arguments);
    assert_eq!(preview.status, 1, "{}", preview.stderr);
    let report = preview.json();
    assert_eq!(report["completion"], "Complete");
    assert_eq!(fixture.bytes("fixing.luau"), original);
    assert_eq!(
        report["proposals"]
            .as_array()
            .expect("validated proposal")
            .len(),
        1
    );
    assert!(!report["previews"][0].as_str().expect("patch").is_empty());
    let mut write_arguments = arguments.to_vec();
    write_arguments.extend(["--fix", "--fix-rule", "self_assignment"]);
    let scoped = fixture.run(&write_arguments);
    assert_eq!(
        scoped.status, 1,
        "unfixed warning still fails: {}",
        scoped.stderr
    );
    assert!(!fixture.read("fixing.luau").contains("value = value"));
    assert!(fixture.read("fixing.luau").contains("if (flag) then"));
    assert_eq!(scoped.json()["counts"]["fixes"], 1);
    assert_eq!(
        scoped.json()["diagnostics"]
            .as_array()
            .expect("remaining findings")
            .len(),
        1
    );
    fixture.write("fixing.luau", &original);
    write_arguments.truncate(arguments.len());
    write_arguments.push("--fix");
    let fixed = fixture.run(&write_arguments);
    assert_eq!(
        fixed.status, 0,
        "final source determines outcome: {}",
        fixed.stderr
    );
    let report = fixed.json();
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["diagnostics"], json!([]));
    assert_eq!(report["counts"]["fixes"], 2);
    assert_eq!(report["counts"]["written"], 1);
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
    let output = fixture.run(&[
        "lint",
        "fixing.luau",
        "second.luau",
        "--json",
        "--rule",
        "self_assignment",
        "--rule",
        "parenthesized_conditions",
        "--fix",
        "--warnings-as-errors",
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.json()["counts"]["written"], 2);
    assert_eq!(output.json()["counts"]["fixes"], 4);
    assert_eq!(fixture.read("fixing.luau"), fixture.read("fixed.luau"));
    assert_eq!(fixture.read("second.luau"), fixture.read("fixed.luau"));
    fixture.write("fixing.luau", &original);
    let source = String::from_utf8(original.clone()).expect("Luau source");
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
            "--json",
            "--output",
            "report.json",
            "--warnings-as-errors",
        ],
        &source,
    );
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.stdout, fixture.read("fixed.luau"));
    assert_eq!(fixture.bytes("fixing.luau"), original);
    let report: Value =
        serde_json::from_str(&fixture.read("report.json")).expect("report destination");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["written"], 0);
    assert_eq!(report["diagnostics"], json!([]));
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
    let default = fixture.run(&["lint", "--json", "--explain-selection"]);
    assert_eq!(default.status, 0, "{}", default.stderr);
    assert_eq!(default.json()["counts"]["selected"], 3);
    assert!(
        default.json()["counts"]["excluded"]
            .as_u64()
            .expect("exclusions")
            > 0
    );
    assert!(
        default.json()["selection"]
            .as_array()
            .expect("candidate explanations")
            .iter()
            .all(|decision| {
                decision["selected"].is_boolean()
                    && !decision["reason"]
                        .as_str()
                        .expect("selection reason")
                        .is_empty()
            })
    );
    let combined = fixture.run(&[
        "lint",
        "selected/one.luau",
        "selected/nested",
        "selected/*.lua",
        "--json",
    ]);
    assert_eq!(combined.status, 0);
    assert_eq!(combined.json()["counts"]["selected"], 3);
    let explicit = fixture.run(&["lint", "selected/ignored.luau", "--json"]);
    assert_eq!(explicit.status, 0);
    assert_eq!(explicit.json()["counts"]["selected"], 1);
    let excluded = fixture.run(&["lint", "selected/excluded.luau", "--json"]);
    assert_eq!(excluded.status, 0);
    assert_eq!(excluded.json()["counts"]["selected"], 0);
    assert!(
        excluded.json()["selection"]
            .as_array()
            .expect("empty selection reasons")
            .iter()
            .any(|decision| {
                decision["selected"] == false
                    && decision["input"]
                        .as_str()
                        .is_some_and(|input| input.ends_with("excluded.luau"))
                    && !decision["reason"]
                        .as_str()
                        .expect("exclusion reason")
                        .is_empty()
            })
    );
    let overridden = fixture.run(&[
        "lint",
        "selected/excluded.luau",
        "other.luau",
        "--include",
        "**/*.luau",
        "--override-exclusions",
        "--json",
    ]);
    assert_eq!(overridden.status, 0);
    assert_eq!(overridden.json()["counts"]["selected"], 2);
    let stdin = fixture.stdin(
        &[
            "lint",
            "-",
            "selected/one.luau",
            "--stdin-filepath",
            "selected/memory.lua",
            "--json",
        ],
        &source,
    );
    assert_eq!(stdin.status, 0);
    assert_eq!(stdin.json()["counts"]["selected"], 2);
    assert!(
        stdin.json()["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|finding| finding["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("memory.lua")))
    );
    for input in ["missing.luau", "selected/*.missing"] {
        let output = fixture.run(&["lint", input, "--json"]);
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
        let mut arguments = vec!["lint", "fixing.luau", "--json"];
        arguments.extend_from_slice(extra);
        let output = fixture.run(&arguments);
        assert_eq!(output.status, 2, "{extra:?}: {}", output.stderr);
        assert_eq!(fixture.bytes("fixing.luau"), original);
    }
    let collision = fixture.run(&[
        "lint",
        "fixing.luau",
        "--json",
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
    let report = fixture.run(&[
        "lint",
        "positive.luau",
        "--json",
        "--rule",
        "self_assignment",
    ]);
    let finding = diagnostic(&report.json(), "self_assignment", "positive.luau", [17, 22]).clone();
    let repeated = fixture.run(&[
        "lint",
        "positive.luau",
        "--report",
        "json",
        "--rule",
        "self_assignment",
        "--jobs",
        "1",
    ]);
    assert_eq!(repeated.status, report.status);
    assert_eq!(
        diagnostic(
            &repeated.json(),
            "self_assignment",
            "positive.luau",
            [17, 22]
        )["identity"],
        finding["identity"]
    );
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
        "--json",
        "--streaming",
        "--rule",
        "self_assignment",
    ]);
    assert_eq!(streaming.status, 0);
    let records: Vec<Value> = streaming
        .stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("streamed JSON record"))
        .collect();
    assert!(records.len() > 1);
    assert!(
        records
            .iter()
            .any(|record| record.to_string().contains("self_assignment"))
    );
    assert_eq!(records.last().expect("terminal record")["event"], "report");
    let terminal = &records.last().expect("terminal report")["report"];
    assert_eq!(terminal["completion"], "Complete");
    assert_eq!(terminal["status"], 0);
    assert_eq!(terminal["counts"]["findings"], 1);
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
    let inherited = fixture.run(&[
        "lint",
        "fixing.luau",
        "child/input.luau",
        "--json",
        "--explain-settings",
        "--project-root",
        ".",
    ]);
    assert_eq!(inherited.status, 0);
    let report = inherited.json();
    assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 2);
    assert!(
        report["settings"]
            .as_array()
            .expect("settings")
            .iter()
            .all(|settings| !settings["origins"].as_array().expect("origins").is_empty())
    );
    fixture.write("alternate.toml", "[linting]\npresets = ['none']\n");
    let explicit = fixture.run(&[
        "lint",
        "fixing.luau",
        "--json",
        "--configuration",
        "alternate.toml",
    ]);
    assert_eq!(explicit.status, 0);
    assert_eq!(explicit.json()["diagnostics"], json!([]));
    fixture.write("child/instar.toml", "");
    fixture.write(
        "child/.luaurc",
        r#"{"languageMode":"strict","lint":{"LocalUnused":false},"globals":["external"]}"#,
    );
    fixture.write("child/input.luau", fixture.bytes("globals.luau"));
    let language = fixture.run(&[
        "lint",
        "child/input.luau",
        "--json",
        "--rule",
        "UnknownGlobal",
    ]);
    assert_eq!(language.status, 0);
    assert_eq!(language.json()["diagnostics"], json!([]));
    fixture.write(
        "executable.luau",
        "return { linting = { selected = { 'self_assignment' } } }\n",
    );
    let executable = fixture.run(&[
        "lint",
        "fixing.luau",
        "--json",
        "--configuration",
        "executable.luau",
    ]);
    assert_eq!(executable.status, 0);
    assert_eq!(
        executable.json()["diagnostics"]
            .as_array()
            .expect("configured rule")
            .len(),
        1
    );
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
    let incomplete = |report: &Value| {
        matches!(
            report["completion"].as_str(),
            Some("TimedOut" | "Cancelled")
        ) || report["completion"]["Incomplete"]
            .as_array()
            .is_some_and(|reasons| !reasons.is_empty())
    };
    for configuration in ["sandbox.luau", "budget.luau"] {
        let mut arguments = vec![
            "lint",
            "fixing.luau",
            "--json",
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
        let report = output.json();
        assert_ne!(
            report["completion"], "Complete",
            "{configuration}: {report}"
        );
        assert!(incomplete(&report), "{configuration}: {report}");
        let sandbox = if configuration == "sandbox.luau" {
            let finding = diagnostic(
                &report,
                "configuration",
                configuration,
                [0, fixture.bytes(configuration).len()],
            );
            assert_eq!(finding["category"], "Configuration");
            assert_eq!(finding["severity"], "error");
            assert!(
                !finding["message"]
                    .as_str()
                    .expect("sandbox explanation")
                    .is_empty()
            );
            Some(json!({
                "code": finding["code"], "category": finding["category"],
                "path": finding["path"], "range": finding["range"],
                "severity": finding["severity"],
            }))
        } else {
            None
        };
        reports.push(fixture.normalize(&json!({
            "case": configuration, "failed": output.status != 0,
            "incomplete": incomplete(&report), "sandbox": sandbox,
        })));
    }
    let output = fixture.run(&[
        "lint",
        "endless.luau",
        "--json",
        "--rule",
        "mismatched_argument_count",
        "--timeout",
        "1",
    ]);
    assert_ne!(output.status, 0);
    let report = output.json();
    assert_ne!(report["completion"], "Complete");
    assert!(incomplete(&report), "{report}");
    reports.push(json!({
        "case": "functions", "failed": output.status != 0,
        "incomplete": incomplete(&report),
    }));
    snapshot(&fixture, "lint", "resources", &json!(reports));
}

#[test]
fn dependencies_are_linted_even_when_excluded() {
    let fixture = Fixture::new("cli", "lint");
    let output = fixture.run(&[
        "lint",
        "imported.luau",
        "--json",
        "--rule",
        "unused_variable",
        "--rule",
        "mismatched_argument_count",
        "--exclude",
        "dependency.luau",
        "--explain-dependencies",
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    let dependency = fixture.read("dependency.luau");
    let start = dependency.find("unused").expect("dependency binding");
    diagnostic(
        &report,
        "unused_variable",
        "dependency.luau",
        [start, start + 6],
    );
    let source = fixture.read("imported.luau");
    let start = source.find("dependency()").expect("resolved call");
    diagnostic(
        &report,
        "mismatched_argument_count",
        "imported.luau",
        [start, start + 12],
    );
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["dependencies"], 1);
    assert_eq!(report["completion"], "Complete");
}

#[test]
fn import_restrictions_and_shadowed_builtins_use_actual_bindings() {
    let fixture = Fixture::new("cli", "lint");
    let setting = r#"linting.rules.restricted_module_paths={"enabled":true,"severity":"Warning","settings":{"rule":"restricted_module_paths","parameters":{"paths":{"./dependency":"use the public module"}}}}"#;
    let output = fixture.run(&[
        "lint",
        "restricted.luau",
        "--json",
        "--rule",
        "non_const_require",
        "--rule",
        "restricted_module_paths",
        "--set",
        setting,
    ]);
    let report = output.json();
    let source = fixture.read("restricted.luau");
    let start = source.find("path").expect("dynamic require argument");
    diagnostic(
        &report,
        "non_const_require",
        "restricted.luau",
        [start, start + 4],
    );
    let text = "'./dependency'";
    let start = source.find(text).expect("restricted require argument");
    let finding = diagnostic(
        &report,
        "restricted_module_paths",
        "restricted.luau",
        [start, start + text.len()],
    );
    assert_eq!(
        finding["message"],
        "module path ./dependency is restricted: use the public module"
    );

    let fixture = Fixture::new("lint", "shadowing");
    for (name, rule) in [
        ("protection.luau", "ignored_pcall_result"),
        ("function.luau", "mismatched_argument_count"),
    ] {
        let original = fixture.bytes(name);
        let output = fixture.run(&["lint", name, "--json", "--rule", rule]);
        assert!(
            output.json()["diagnostics"]
                .as_array()
                .expect("findings")
                .iter()
                .all(|finding| finding["code"] != rule)
        );
        assert_eq!(fixture.bytes(name), original);
    }
}

#[test]
fn source_pragmas_disable_type_diagnostics_without_disabling_semantic_rules() {
    let fixture = Fixture::new("cli", "lint");
    let output = fixture.run(&[
        "lint",
        "pragmas.luau",
        "--json",
        "--rule",
        "implicit_any_parameter",
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    let source = fixture.read("pragmas.luau");
    let start = source
        .find("parameter")
        .expect("contextually inferred any parameter");
    diagnostic(
        &report,
        "implicit_any_parameter",
        "pragmas.luau",
        [start, start + 9],
    );
    assert_eq!(report["diagnostics"].as_array().expect("findings").len(), 1);
    assert_eq!(report["completion"], "Complete");
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
        let output = fixture.run(&[
            "lint",
            name,
            "--json",
            "--rule",
            rule,
            "--set",
            "environment.roblox=true",
            "--set",
            &directory,
        ]);
        assert_eq!(output.status, 0, "{}", output.stderr);
        let report = output.json();
        assert_eq!(report["completion"], "Complete");
        if let Some(range) = range {
            diagnostic(&report, rule, name, range);
            assert_eq!(
                report["diagnostics"]
                    .as_array()
                    .expect("paired finding")
                    .len(),
                1
            );
        } else {
            assert_eq!(report["diagnostics"], json!([]));
        }
        reports.push(fixture.normalize(&json!({
            "scenario": scenario, "rule": rule, "diagnostics": report["diagnostics"],
            "sources": report["sources"], "availability": report["availability"],
            "counts": report["counts"], "completion": report["completion"], "status": report["status"],
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
    assert!(changed.final_findings.is_empty());
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
    assert!(!source.iterations[0].changed.is_empty());
    let WatchCommandReport::Lint(changed) = &source.iterations[0].report else {
        panic!("watch must return lint report");
    };
    assert!(changed.final_findings.is_empty());
    fixture.write("instar.toml", "[linting]\nselected = ['implicit_any_parameter']\n[[environment.groups]]\nnamespace = '@project'\ndefinitions = ['declaration.luau']\n");
    let rules =
        complete(project.watch(request.clone(), options())).expect("configuration-triggered watch");
    assert_eq!(rules.iterations.len(), 1);
    assert!(!rules.iterations[0].changed.is_empty());
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
    assert!(!environment.iterations[0].changed.is_empty());
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
        assert!(!message.is_empty());
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
    let output = fixture.run(&[
        "lint",
        "shared.luau",
        "--json",
        "--rule",
        "mismatched_argument_count",
        "--explain-settings",
        "--explain-dependencies",
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    assert_eq!(report["completion"], "Complete");
    let source = fixture.read("shared.luau");
    for call in ["input.count(1)", "library.double(1, 2)", "documented()"] {
        let start = source.find(call).expect("shared resolved call");
        diagnostic(
            &report,
            "mismatched_argument_count",
            "shared.luau",
            [start, start + call.len()],
        );
    }
    assert_eq!(
        report["diagnostics"]
            .as_array()
            .expect("real signature findings")
            .len(),
        3
    );
    assert_eq!(report["counts"]["dependencies"], 2);
    for settings in report["settings"].as_array().expect("settings") {
        let groups = settings["settings"]["environment"]["groups"]
            .as_array()
            .expect("ordered declaration groups");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0]["namespace"], "@contract");
        assert_eq!(groups[1]["namespace"], "@additional");
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
    let output = fixture.run(&[
        "lint",
        "shared.luau",
        "--json",
        "--rule",
        "divide_by_zero",
        "--explain-dependencies",
        "--set",
        &directory,
    ]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    assert_eq!(report["completion"], "Complete");
    diagnostic(&report, "divide_by_zero", "shared.luau", [40, 41]);
    diagnostic(&report, "divide_by_zero", "first.luau", [9, 10]);
    diagnostic(&report, "divide_by_zero", "second.luau", [9, 10]);
    assert_eq!(
        report["diagnostics"]
            .as_array()
            .expect("physical findings")
            .len(),
        3
    );
    assert_eq!(
        report["modules"].as_array().expect("module contexts").len(),
        4
    );
    assert_eq!(
        report["sources"]
            .as_array()
            .expect("physical sources")
            .len(),
        3
    );
    assert_eq!(
        report["dependencies"]
            .as_array()
            .expect("resolved edges")
            .len(),
        2
    );
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
    let output = fixture.run(&["lint", "positive.luau", "--json", "--rule", "LocalShadow"]);
    assert_eq!(output.status, 0, "{}", output.stderr);
    let report = output.json();
    let finding = diagnostic(
        &report,
        "LocalShadow",
        "positive.luau",
        [shadow, shadow + 6],
    );
    assert!(
        finding["related"]
            .as_array()
            .expect("secondary declaration locations")
            .iter()
            .any(|related| {
                related["range"] == json!([original, original + 6])
                    && related["source"] == finding["source"]
                    && related["path"] == finding["path"]
                    && !related["message"]
                        .as_str()
                        .expect("declaration explanation")
                        .is_empty()
            })
    );
}

#[test]
fn operational_logging_is_separate_from_the_machine_readable_report() {
    let fixture = Fixture::new("lint", "division");
    let output = fixture.run(&[
        "lint",
        "positive.luau",
        "--json",
        "--rule",
        "divide_by_zero",
        "--verbose",
    ]);
    assert_eq!(output.status, 0);
    let report = output.json();
    diagnostic(&report, "divide_by_zero", "positive.luau", [12, 16]);
    let logs: Vec<Value> = output
        .stderr
        .lines()
        .map(|line| serde_json::from_str(line).expect("operational JSON log"))
        .collect();
    assert!(!logs.is_empty());
    assert!(
        logs.iter()
            .all(|record| record.get("diagnostics").is_none())
    );
    assert!(logs.iter().any(|record| record["level"] == "DEBUG"));
}

#[test]
fn immutable_binding_exemptions_respect_actual_shadowed_mutation() {
    let fixture = Fixture::new("lint", "shadowing");
    let setting = r#"linting.rules.prefer_const={"enabled":true,"severity":"Warning","settings":{"rule":"prefer_const","parameters":{"mutated_tables_stay_local":true}}}"#;
    let shadow = fixture.run(&[
        "lint",
        "table.luau",
        "--json",
        "--rule",
        "prefer_const",
        "--set",
        setting,
    ]);
    assert_eq!(shadow.status, 0);
    let report = shadow.json();
    diagnostic(&report, "prefer_const", "table.luau", [6, 7]);
    let mutation = fixture.run(&[
        "lint",
        "mutation.luau",
        "--json",
        "--rule",
        "prefer_const",
        "--set",
        setting,
    ]);
    assert_eq!(mutation.status, 0);
    assert_eq!(mutation.json()["diagnostics"], json!([]));
    let fixture = Fixture::new("lint", "escapes");
    let bytes = fixture.run(&["lint", "byte.luau", "--json", "--rule", "bad_string_escape"]);
    assert_eq!(bytes.status, 0);
    assert_eq!(bytes.json()["diagnostics"], json!([]));
}

#[test]
fn declared_globals_and_warning_modes_follow_source_policy() {
    let fixture = Fixture::new("cli", "lint");
    let source = fixture.read("positive.luau");
    for (mode, unknown, implicit) in [("strict", 0, 0), ("nonstrict", 0, 1), ("nocheck", 1, 1)] {
        fixture.write(".luaurc", json!({"languageMode": mode}).to_string());
        let output = fixture.run(&[
            "lint",
            "positive.luau",
            "--json",
            "--rule",
            "UnknownGlobal",
            "--rule",
            "ImplicitReturn",
        ]);
        assert_eq!(output.status, 0, "{mode}: {}", output.stdout);
        let report = output.json();
        assert_eq!(report["completion"], "Complete");
        assert_eq!(
            report["diagnostics"].as_array().expect("warnings").len(),
            unknown + implicit
        );
        let findings: Vec<_> = report["diagnostics"]
            .as_array()
            .expect("warnings")
            .iter()
            .filter(|finding| finding["code"] == "UnknownGlobal")
            .collect();
        assert_eq!(findings.len(), unknown, "{mode}");
        if unknown > 0 {
            let start = source.find("missing").expect("unknown reference");
            diagnostic(
                &report,
                "UnknownGlobal",
                "positive.luau",
                [start, start + 7],
            );
        }
        if implicit > 0 {
            let text = "if flag then return 1 end";
            let start = source.find(text).expect("fallthrough branch");
            diagnostic(
                &report,
                "ImplicitReturn",
                "positive.luau",
                [start, start + text.len()],
            );
        }
    }
    fixture.write(".luaurc", r#"{"languageMode":"nocheck"}"#);
    let output = fixture.run(&[
        "lint",
        "positive.luau",
        "--json",
        "--rule",
        "UnknownGlobal",
        "--set",
        r#"environment.globals=["missing"]"#,
    ]);
    assert_eq!(output.status, 0);
    assert_eq!(output.json()["diagnostics"], json!([]));
    let deprecated = fixture.run(&[
        "lint",
        "negative.luau",
        "--json",
        "--rule",
        "DeprecatedGlobal",
    ]);
    assert_eq!(deprecated.status, 0);
    assert_eq!(deprecated.json()["completion"], "Complete");
    assert_eq!(deprecated.json()["diagnostics"], json!([]));
}
