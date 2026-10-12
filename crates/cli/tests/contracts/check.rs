use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use instar_luau::{ImportOutcome, flags::FlagValue};

use instar_project::{
    ChangeBatch, CheckReport, CheckRequest, CheckingMode, DeclarationSettings, DiagnosticPolicy,
    Input, InputIdentity, Invocation, Project, ProjectChange, SourceOrigin, WatchCommandReport,
    WatchOperation, WatchRequest,
};

use instar_shared::{
    Completion, DiagnosticCategory, EventPayload, FailureKind, OperationFailure, Options, Severity,
};

use serde_json::json;

use super::support::{Fixture, attributed, complete, diagnostic, options, snapshot};

#[test]
fn cancellation() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let project = fixture.project();
    let mut execution = options();
    execution.jobs = NonZeroUsize::MIN;
    let cancellation = execution.cancellation.clone();

    execution.observer = Some(Arc::new(move |event| {
        if matches!(&event.payload, EventPayload::Diagnostic(diagnostic) if diagnostic.code == "1000")
        {
            cancellation.cancel();
        }
    }));

    let failure = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(fixture.path("probe.luau"))],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        execution,
    ))
    .expect_err("interrupted analysis cannot succeed");

    complete(project.finish()).expect("finish cancelled project");

    assert_eq!(failure.completion, Completion::Cancelled);

    let diagnostic = failure
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "1000")
        .expect("retained partial finding");

    assert_eq!(diagnostic.category, DiagnosticCategory::Type);
    let captured = project.snapshot();

    let source = captured
        .source(diagnostic.primary.source.identity)
        .expect("diagnostic source");

    assert_eq!(source.version, diagnostic.primary.source);

    assert_eq!(
        &source.text[diagnostic.primary.span.start..diagnostic.primary.span.end],
        "12"
    );

    assert_eq!(fixture.read("probe.luau"), source.text.as_ref());

    snapshot(
        &fixture,
        "check",
        "cancellation",
        &json!({
            "completion": failure.completion,
            "diagnostics": attributed(&failure.diagnostics, &captured),
            "completed_sources": failure.completed_sources,
        }),
    );
}

fn check(fixture: &Fixture, request: CheckRequest) -> Result<CheckReport, Box<OperationFailure>> {
    let project = fixture.project();
    let report = complete(project.check(request, options()));
    complete(project.finish()).expect("finish contract project");

    report.map_err(Box::new)
}

#[test]
fn closure() {
    let fixture = Fixture::new("cli", "project");

    let originals = [
        "input.luau",
        "consumer.lua",
        "library.luau",
        "types.luau",
        "instar.toml",
    ]
    .map(|name| (name, fixture.bytes(name)));

    let output = fixture.run(&[
        "check",
        "consumer.lua",
        "--explain-dependencies",
        "--jobs",
        "1",
    ]);

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert!(output.stderr.contains("Dependencies:"));
    assert!(output.stderr.contains("Imports:"));
    assert!(output.stderr.contains("./library"));
    let project = fixture.project();
    let mut execution = options();
    execution.jobs = NonZeroUsize::MIN;
    let invocation = request(&fixture, &["consumer.lua"]);

    let report =
        complete(project.check(invocation.clone(), execution.clone())).expect("serial closure");

    complete(project.finish()).expect("finish serial closure project");
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.dependencies, 2);
    assert_eq!(report.counts.findings, 0);
    assert_eq!(report.sources.len(), 3);
    assert_eq!(report.findings, []);
    assert_eq!(report.dependencies.len(), 3);
    let parallel = fixture.run(&["check", "consumer.lua", "--jobs", "2"]);
    assert_eq!(parallel.status, 0, "{}", parallel.stderr);
    execution.jobs = NonZeroUsize::new(2).expect("parallel jobs");
    let parallel_project = fixture.project();

    let parallel_report =
        complete(parallel_project.check(invocation, execution)).expect("parallel closure");

    complete(parallel_project.finish()).expect("finish parallel closure project");
    assert_eq!(json!(parallel_report.counts), json!(report.counts));
    assert_eq!(parallel_report.findings, report.findings);

    assert_eq!(
        json!(parallel_report.dependencies),
        json!(report.dependencies)
    );

    let invalid_jobs = fixture.run(&["check", "consumer.lua", "--jobs", "0", "--color", "never"]);
    assert_eq!(invalid_jobs.status, 2);
    assert!(invalid_jobs.stderr.contains("invalid value"));

    for (name, bytes) in originals {
        assert_eq!(fixture.bytes(name), bytes, "checking changed {name}");
    }

    snapshot(
        &fixture,
        "check",
        "closure",
        &json!({
            "sources": report.sources,
            "modules": report.modules,
            "dependencies": report.dependencies,
            "counts": report.counts,
            "completion": report.completion,
        }),
    );
}

#[test]
fn configuration() {
    let fixture = Fixture::empty();

    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=false\nglobals=['provided']\n",
    );

    fixture.write("nested/instar.toml", "[checking]\nmode='nocheck'\n");

    fixture.write(
        "nested/.luaurc",
        "{\"languageMode\":\"strict\",\"globals\":[\"child\"]}\n",
    );

    fixture.write(
        "nested/input.luau",
        "local value: string = 12\nreturn value\n",
    );

    let configuration = fixture.bytes("instar.toml");
    let child = fixture.bytes("nested/instar.toml");
    let output = fixture.run(&["check", "nested/input.luau"]);

    let report = check(&fixture, request(&fixture, &["nested/input.luau"]))
        .expect("inherited configuration");

    let effective = &report.settings[0];
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(effective.settings.checking, CheckingMode::Nocheck);
    assert_eq!(effective.settings.environment.globals, ["child"]);
    let overridden = fixture.run(&["check", "nested/input.luau", "--mode", "strict"]);
    let mut invocation = request(&fixture, &["nested/input.luau"]);
    invocation.overrides.checking.mode = Some(CheckingMode::Strict);
    let strict = check(&fixture, invocation).expect("strict override");
    assert_eq!(overridden.status, 1, "{}", overridden.stderr);
    assert_eq!(strict.settings[0].settings.checking, CheckingMode::Strict);
    assert_eq!(strict.findings[0].category, DiagnosticCategory::Type);
    assert_eq!(fixture.bytes("instar.toml"), configuration);
    assert_eq!(fixture.bytes("nested/instar.toml"), child);

    snapshot(
        &fixture,
        "check",
        "configuration",
        &json!({
            "inherited": {
                "mode": effective.settings.checking,
                "globals": effective.settings.environment.globals,
            },
            "override": {"mode": strict.settings[0].settings.checking, "findings": attributed(&strict.findings, &strict.snapshot)},
        }),
    );
}

#[test]
fn cycles() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[language.flags]\nLuauCyclicRequireTypeInference=true\nLuauExportValueSyntax=true\nLuauExportValueTypecheck=true\n");
    fixture.write("first.luau", "local second = require('./second')\nexport function first(value: number): number\n    return if value == 0 then 0 else second.second(value - 1)\nend\n");
    fixture.write("second.luau", "local first = require('./first')\nexport function second(value: number): number\n    return if value == 0 then 0 else first.first(value - 1)\nend\n");

    fixture.write(
        "input.luau",
        "local first = require('./first')\nlocal value: number = first.first(3)\nreturn value\n",
    );

    let valid = fixture.run(&["check", "input.luau"]);
    let report = check(&fixture, request(&fixture, &["input.luau"])).expect("inferrable cycle");
    assert_eq!(valid.status, 0, "{}", valid.stderr);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.dependencies, 2);
    assert_eq!(report.dependencies.len(), 3);

    fixture.write(
        "first.luau",
        "local second = require('./second')\nreturn second\n",
    );

    fixture.write(
        "second.luau",
        "local first = require('./first')\nreturn first\n",
    );

    let invalid = fixture.run(&["check", "first.luau", "--explain-dependencies"]);
    let failed = check(&fixture, request(&fixture, &["first.luau"])).expect("cycle findings");
    assert_eq!(invalid.status, 1, "{}", invalid.stderr);

    let cycle = failed
        .findings
        .iter()
        .find(|finding| finding.code == "1024")
        .expect("uninferrable cycle diagnostic");

    for path in ["first.luau", "second.luau"] {
        assert!(cycle.related.iter().any(|related| {
            failed
                .snapshot
                .origin(related.location.source.identity)
                .and_then(|origin| origin.path())
                == Some(fixture.path(path).as_path())
        }));
    }

    assert!(
        cycle
            .related
            .iter()
            .all(|related| related.message.contains("requires"))
    );
}

#[test]
fn declarations() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("probe.luau", "--!strict\nlocal inherited: number = subject.value\nlocal declared: string = subject.label\nlocal builtin: number = math.abs(-3)\nlocal length: number = string.len(declared)\nreturn inherited + builtin + length\n");
    let valid = fixture.run(&["check", "probe.luau"]);
    let report = check(&fixture, request(&fixture, &["probe.luau"])).expect("declarations");
    assert_eq!(valid.status, 0, "{}", valid.stderr);
    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.findings, 0);
    assert_eq!(report.completion, Completion::Complete);

    fixture.write(
        "probe.luau",
        "--!strict\nlocal inherited: string = subject.value\nreturn inherited\n",
    );

    let invalid = fixture.run(&["check", "probe.luau"]);

    let mismatch =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("declaration mismatch");

    assert_eq!(invalid.status, 1, "{}", invalid.stderr);
    assert_eq!(mismatch.findings[0].category, DiagnosticCategory::Type);
    assert!(mismatch.findings[0].message.contains("number"));
    fixture.write("probe.luau", "--!strict\nreturn provided\n");

    let globals = fixture.run(&[
        "check",
        "probe.luau",
        "--set",
        "environment.globals=[\"provided\"]",
    ]);

    assert_eq!(globals.status, 0, "{}", globals.stderr);
    let unknown = fixture.run(&["check", "probe.luau", "--set", "environment.globals=[]"]);
    let mut invocation = request(&fixture, &["probe.luau"]);
    invocation.overrides.environment.globals = Some(Vec::new());
    let unknown_report = check(&fixture, invocation).expect("unknown global findings");
    assert_eq!(unknown.status, 1, "{}", unknown.stderr);

    assert!(
        unknown_report
            .findings
            .iter()
            .any(|finding| finding.message.contains("provided"))
    );
}

#[test]
fn dependencies() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[selection.checking]\nexclude=['library/**']\n[resolution.aliases]\npackage='library'\n");
    fixture.write("library/init.luau", "export type Record = {value: number}\nlocal function double(value: number): number\n    return value * 2\nend\nreturn {double = double}\n");
    fixture.write("nested/input.luau", "local library = require('@package')\nlocal relative = require('../library')\nlocal record: library.Record = {value = relative.double(2)}\nlocal value: number = record.value\nreturn value\n");

    let clean = fixture.run(&["check", "nested/input.luau", "--explain-dependencies"]);

    let report =
        check(&fixture, request(&fixture, &["nested/input.luau"])).expect("dependency closure");

    assert_eq!(clean.status, 0, "{}", clean.stderr);
    assert!(clean.stderr.contains("@package"));
    assert!(clean.stderr.contains("Dependencies:"));
    assert!(clean.stderr.contains("Imports:"));
    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.dependencies, 1);
    assert_eq!(report.dependencies.len(), 2);
    assert_eq!(report.imports.len(), 2);
    assert!(report.imports.iter().all(|import| import.failure.is_none()));
    assert_eq!(report.dependencies[0].target, report.dependencies[1].target);
    fixture.write("nested/input.luau", "local library = require('@package')\nlocal record: library.Record = {value = 'wrong'}\nreturn library.double('wrong'), record\n");
    fixture.write("library/init.luau", "export type Record = {value: number}\nlocal value: string = 12\nlocal function double(value: number): number\n    return value * 2\nend\nreturn {double = double, value = value}\n");
    let invalid = fixture.run(&["check", "nested/input.luau"]);

    let findings =
        check(&fixture, request(&fixture, &["nested/input.luau"])).expect("dependency findings");

    assert_eq!(invalid.status, 1, "{}", invalid.stderr);
    assert_eq!(findings.counts.dependencies, 1);

    for path in ["nested/input.luau", "library/init.luau"] {
        assert!(
            findings.findings.iter().any(|finding| {
                finding.category == DiagnosticCategory::Type
                    && findings
                        .snapshot
                        .origin(finding.primary.source.identity)
                        .and_then(|origin| origin.path())
                        == Some(fixture.path(path).as_path())
            }),
            "missing type findings in {path}: {findings:?}"
        );
    }
}

#[test]
fn destination() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let output = fixture.run(&[
        "check",
        "probe.luau",
        "--color",
        "never",
        "--output",
        "report.txt",
    ]);

    let report = fixture.read("report.txt");
    assert_eq!(output.status, 1);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
    assert!(report.contains("1000"));
    assert!(report.contains("probe.luau:2:"));
    assert!(report.contains("local value: string = 12"));
    assert!(report.contains("Checking complete: 1 file"));

    for path in ["probe.luau", "instar.toml", "types.luau"] {
        let bytes = fixture.bytes(path);
        let refused = fixture.run(&["check", "probe.luau", "--output", path]);

        assert_eq!(
            refused.status, 2,
            "input report destination {path}: {}",
            refused.stderr
        );

        assert!(refused.stderr.contains("input"));
        assert_eq!(fixture.bytes(path), bytes, "report clobbered {path}");
    }

    let failed = fixture.run(&["check", "probe.luau", "--output", "absent/report.txt"]);
    assert_eq!(failed.status, 3, "{}", failed.stderr);
    assert_ne!(failed.stderr, "");
    assert!(!failed.stderr.contains("Checking complete"));
    assert!(!fixture.path("absent/report.txt").exists());
}

#[test]
fn environments() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let missing = fixture.run(&[
        "check",
        "probe.luau",
        "--set",
        "environment.groups=[{\"namespace\":\"@missing\",\"definitions\":[\"absent.luau\"]}]",
    ]);

    let mut invocation = request(&fixture, &["probe.luau"]);

    invocation.overrides.environment.groups = Some(vec![DeclarationSettings {
        namespace: "@missing".to_owned(),
        definitions: vec![fixture.path("absent.luau")],
        documentation: Vec::new(),
    }]);

    let report = check(&fixture, invocation).expect("missing environment report");
    assert_eq!(missing.status, 4, "{}", missing.stderr);
    assert_ne!(report.completion, Completion::Complete);

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "environment.unavailable")
    );

    fixture.write("broken.luau", "declare broken: MissingType\n");

    let invalid = fixture.run(&[
        "check",
        "probe.luau",
        "--set",
        "environment.groups=[{\"namespace\":\"@broken\",\"definitions\":[\"broken.luau\"]}]",
    ]);

    let mut invocation = request(&fixture, &["probe.luau"]);

    invocation.overrides.environment.groups = Some(vec![DeclarationSettings {
        namespace: "@broken".to_owned(),
        definitions: vec![fixture.path("broken.luau")],
        documentation: Vec::new(),
    }]);

    let failure = check(&fixture, invocation).expect("invalid environment report");
    assert_eq!(invalid.status, 4, "{}", invalid.stderr);
    assert_ne!(failure.completion, Completion::Complete);

    assert!(failure.findings.iter().any(|finding| {
        failure
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("broken.luau").as_path())
    }));

    let unavailable = failure
        .findings
        .iter()
        .find(|finding| finding.code == "environment.unavailable")
        .expect("unavailable environment");

    assert!(unavailable.related.iter().any(|related| {
        failure
            .snapshot
            .origin(related.location.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("broken.luau").as_path())
    }));

    let roblox = fixture.run(&[
        "check",
        "probe.luau",
        "--set",
        "environment.roblox=true",
        "--set",
        "environment.asset_directory=\"absent\"",
    ]);

    let mut invocation = request(&fixture, &["probe.luau"]);
    invocation.overrides.environment.roblox = Some(true);
    invocation.overrides.environment.asset_directory = Some(fixture.path("absent"));
    let unavailable = check(&fixture, invocation).expect("unavailable Roblox report");
    assert_eq!(roblox.status, 4, "{}", roblox.stderr);
    assert_ne!(unavailable.completion, Completion::Complete);

    assert!(
        unavailable
            .findings
            .iter()
            .any(|finding| finding.code == "environment.unavailable")
    );

    snapshot(
        &fixture,
        "check",
        "environments",
        &json!({
            "missing": {"findings": attributed(&report.findings, &report.snapshot), "completion": report.completion},
            "invalid": {"findings": attributed(&failure.findings, &failure.snapshot), "completion": failure.completion},
            "roblox": {"findings": attributed(&unavailable.findings, &unavailable.snapshot), "completion": unavailable.completion},
        }),
    );
}

#[test]
fn filtering() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let project = fixture.project();

    let report = complete(project.check(request(&fixture, &["probe.luau"]), options()))
        .expect("unfiltered findings");

    assert!(report.failed);
    assert_eq!(report.findings.len(), 1);

    let finding = diagnostic(
        &report.findings,
        &report.snapshot,
        "1000",
        "probe.luau",
        [32, 34],
    );

    assert_eq!(finding.category, DiagnosticCategory::Type);
    assert_eq!(finding.severity, Severity::Error);
    assert_eq!(finding.identity.code(), "1000");
    assert_eq!(finding.identity.producer(), "luau");
    let mut cases = Vec::new();

    for (filter, policy) in [
        (
            ["--diagnostic-code", "absent"],
            DiagnosticPolicy {
                codes: Some(["absent".to_owned()].into()),
                ..DiagnosticPolicy::default()
            },
        ),
        (
            ["--diagnostic-category", "syntax"],
            DiagnosticPolicy {
                categories: Some([DiagnosticCategory::Syntax].into()),
                ..DiagnosticPolicy::default()
            },
        ),
        (
            ["--diagnostic-severity", "warning"],
            DiagnosticPolicy {
                severities: Some([Severity::Warning].into()),
                ..DiagnosticPolicy::default()
            },
        ),
    ] {
        let mut invocation = request(&fixture, &["probe.luau"]);
        invocation.overrides.diagnostics = policy;

        let filtered = complete(project.check(invocation, options())).expect("filtered findings");
        assert!(filtered.failed);
        assert_eq!(filtered.completion, Completion::Complete);
        assert_eq!(filtered.sources, report.sources);
        assert_eq!(filtered.modules, report.modules);
        assert_eq!(filtered.findings, []);
        cases.push(json!({"filter": filter, "counts": filtered.counts, "failed": filtered.failed, "completion": filtered.completion}));
    }

    let mut invocation = request(&fixture, &["probe.luau"]);
    invocation.overrides.diagnostics.codes = Some(["1000".to_owned()].into());

    let included_report =
        complete(project.check(invocation, options())).expect("included findings");

    assert_eq!(included_report.findings, report.findings);
    complete(project.finish()).expect("finish filter project");

    snapshot(
        &fixture,
        "check",
        "filtering",
        &json!({"findings": report.findings, "filtered": cases}),
    );
}

#[test]
fn filtering_arguments() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let output = fixture.run(&["check", "probe.luau", "--color", "never"]);
    assert_eq!(output.status, 1, "{}", output.stderr);
    assert_eq!(output.stderr.matches("error[1000]").count(), 1);

    for arguments in [
        ["--diagnostic-code", "absent"],
        ["--diagnostic-category", "syntax"],
        ["--diagnostic-severity", "warning"],
    ] {
        let output = fixture.run(&[
            "check",
            "probe.luau",
            arguments[0],
            arguments[1],
            "--color",
            "never",
        ]);

        assert_eq!(output.status, 1, "{}", output.stderr);
        assert!(!output.stderr.contains("error[1000]"));
        assert!(!output.stderr.contains("local value: string = 12"));
    }

    let included = fixture.run(&[
        "check",
        "probe.luau",
        "--diagnostic-code",
        "1000",
        "--color",
        "never",
    ]);

    assert_eq!(included.status, 1);
    assert!(included.stderr.contains("1000"));
}

#[test]
fn groups() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[[environment.groups]]\nnamespace='@base'\ndefinitions=['base.luau']\ndocumentation=['base.json']\n[[environment.groups]]\nnamespace='@child'\ndefinitions=['child.luau']\ndocumentation=['child.json']\n");

    fixture.write(
        "base.luau",
        "declare extern type Base with\n    value: number\nend\n",
    );

    fixture.write("child.luau", "declare extern type Child extends Base with\n    label: string\nend\ndeclare subject: Child\n");
    fixture.write("input.luau", "local value: number = subject.value\nlocal label: string = subject.label\nreturn value, label\n");

    fixture.write(
        "base.json",
        "{\"@base/type/Base\":{\"documentation\":\"Base contract\"}}\n",
    );

    fixture.write(
        "child.json",
        "{\"@child/global/subject\":{\"documentation\":\"Child contract\"}}\n",
    );

    let ordered = fixture.run(&["check", "input.luau"]);
    let report = check(&fixture, request(&fixture, &["input.luau"])).expect("ordered declarations");
    assert_eq!(ordered.status, 0, "{}", ordered.stderr);

    assert_eq!(
        report.settings[0].settings.environment.groups[0].namespace,
        "@base"
    );

    assert_eq!(
        report.settings[0].settings.environment.groups[1].namespace,
        "@child"
    );

    let reversed = fixture.run(&[
        "check", "input.luau", "--set",
        "environment.groups=[{\"namespace\":\"@child\",\"definitions\":[\"child.luau\"]},{\"namespace\":\"@base\",\"definitions\":[\"base.luau\"]}]",
    ]);

    let mut invocation = request(&fixture, &["input.luau"]);

    invocation.overrides.environment.groups = Some(
        [("@child", "child.luau"), ("@base", "base.luau")]
            .map(|(namespace, path)| DeclarationSettings {
                namespace: namespace.to_owned(),
                definitions: vec![fixture.path(path)],
                documentation: Vec::new(),
            })
            .into(),
    );

    let unavailable = check(&fixture, invocation).expect("reversed environment report");
    assert_eq!(reversed.status, 4, "{}", reversed.stderr);
    assert_ne!(unavailable.completion, Completion::Complete);

    assert!(unavailable.findings.iter().any(|finding| {
        unavailable
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("child.luau").as_path())
    }));

    fixture.write(
        "child.json",
        "{\"@base/global/subject\":{\"documentation\":\"Wrong namespace\"}}\n",
    );

    let mismatched = fixture.run(&["check", "input.luau"]);

    let attribution =
        check(&fixture, request(&fixture, &["input.luau"])).expect("documentation attribution");

    assert_eq!(mismatched.status, 4, "{}", mismatched.stderr);
    assert_ne!(attribution.completion, Completion::Complete);

    assert!(attribution.findings.iter().any(|finding| {
        attribution
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("child.json").as_path())
    }));
}

#[test]
fn inputs() {
    let fixture = Fixture::empty();

    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=false\n",
    );

    fixture.write(
        "source/first.luau",
        "local value: number = 1\nreturn value\n",
    );

    fixture.write(
        "source/nested/second.lua",
        "local value: string = 'second'\nreturn value\n",
    );

    fixture.write("third.luau", "local value: boolean = true\nreturn value\n");
    let mut cases = Vec::new();

    for (name, arguments, selected) in [
        ("file", vec!["source/first.luau"], 1),
        ("directory", vec!["source"], 2),
        ("pattern", vec!["source/**/*.lua"], 1),
        (
            "multiple",
            vec!["third.luau", "source", "source/**/*.lua"],
            3,
        ),
        ("default", Vec::new(), 3),
    ] {
        let mut command = vec!["check", "--explain-selection"];
        command.extend(arguments.iter().copied());
        let output = fixture.run(&command);
        let mut invocation = request(&fixture, &[]);

        invocation.invocation.inputs = arguments
            .iter()
            .map(|path| {
                if path.contains('*') {
                    Input::Pattern {
                        base: fixture.root().to_path_buf(),
                        pattern: (*path).to_owned(),
                    }
                } else {
                    Input::Path(fixture.path(path))
                }
            })
            .collect();

        invocation.invocation.explanations.selection = true;
        let report = check(&fixture, invocation).expect("input selection");
        assert_eq!(output.status, 0, "{name}: {}", output.stderr);
        assert!(output.stderr.contains("Selection:"));
        assert_eq!(report.completion, Completion::Complete);
        assert_eq!(report.counts.selected, selected, "{name}");
        assert_eq!(report.counts.dependencies, 0);
        assert_eq!(report.counts.findings, 0);
        cases.push(json!({"case": name, "selection": report.selection, "counts": report.counts, "completion": report.completion}));
    }

    snapshot(&fixture, "check", "inputs", &json!(cases));
}

#[test]
fn invalid() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("binary.luau", [0xFF, 0xFE]);
    fixture.write("text.txt", "return 1\n");
    fixture.write("broken.toml", "[checking]\nmode='impossible'\n");

    for arguments in [
        vec!["missing.luau"],
        vec!["unmatched/**/*.luau"],
        vec!["binary.luau"],
        vec!["text.txt"],
        vec!["input.luau", "--configuration", "missing.toml"],
        vec!["input.luau", "--configuration", "broken.toml"],
        vec!["input.luau", "--include", "["],
        vec!["input.luau", "--set", "checking.unknown=true"],
        vec!["input.luau", "--set", "checking.mode=notjson"],
        vec!["input.luau", "--stdin-filepath", "input.luau"],
    ] {
        let mut command = vec!["check"];
        command.extend(arguments.iter().copied());
        let output = fixture.run(&command);
        assert_eq!(output.status, 2, "{arguments:?}: {}", output.stderr);
        assert_eq!(output.stdout, "");
        assert_ne!(output.stderr.trim(), "");
    }

    let mut failures = Vec::new();

    for (name, invocation) in [
        ("missing", request(&fixture, &["missing.luau"])),
        ("binary", request(&fixture, &["binary.luau"])),
        ("extension", request(&fixture, &["text.txt"])),
    ] {
        let failure = check(&fixture, invocation).expect_err("invalid source input");
        assert_eq!(failure.kind, FailureKind::InvalidInvocation);
        assert_ne!(failure.completion, Completion::Complete);
        assert_ne!(failure.message, "");
        failures.push(json!({"case": name, "failure": failure}));
    }

    for (path, diagnostic_path) in [
        ("missing.toml", "input.luau"),
        ("broken.toml", "broken.toml"),
    ] {
        let project = fixture.project();
        let mut invocation = request(&fixture, &["input.luau"]);
        invocation.invocation.configuration = Some(fixture.path(path));

        let failure = complete(project.check(invocation, options()))
            .expect_err("invalid configuration input");

        complete(project.finish()).expect("finish invalid configuration project");
        assert_eq!(failure.kind, FailureKind::InvalidInvocation);
        assert_ne!(failure.completion, Completion::Complete);
        assert_ne!(failure.message, "");
        let captured = project.snapshot();

        let finding = diagnostic(
            &failure.diagnostics,
            &captured,
            "configuration.invalid",
            diagnostic_path,
            [0, 0],
        );

        assert_eq!(finding.category, DiagnosticCategory::Configuration);
        failures.push(json!({"configuration": path, "failure": failure}));
    }

    snapshot(&fixture, "check", "invalid", &json!(failures));
    let empty = Fixture::empty();
    let output = empty.run(&["check"]);
    assert_eq!(output.status, 2);
    assert!(output.stderr.contains("empty"));
    let duplicated = fixture.stdin(&["check", "-", "-"], "return 1\n");
    assert_eq!(duplicated.status, 2);
    assert!(duplicated.stderr.contains("once"));
}

#[test]
fn mapping() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n[resolution]\nsourcemaps=['sourcemap.json']\n");
    fixture.write("sourcemap.json", "{\"name\":\"Place\",\"className\":\"DataModel\",\"children\":[{\"name\":\"Library\",\"className\":\"Folder\",\"children\":[{\"name\":\"Caller\",\"className\":\"ModuleScript\",\"filePaths\":[\"input.luau\"]},{\"name\":\"Target\",\"className\":\"ModuleScript\",\"filePaths\":[\"target.luau\"]}]}]}\n");
    fixture.write("input.luau", "return require(script.Parent.Target)\n");
    fixture.write("target.luau", "return {value = 1}\n");
    let output = fixture.run(&["check", "input.luau", "--explain-dependencies"]);
    let report = check(&fixture, request(&fixture, &["input.luau"])).expect("mapped closure");
    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.dependencies, 1);
    assert_eq!(report.dependencies.len(), 1);
    assert!(report.imports[0].failure.is_none());
    fixture.write("input.luau", "return require(script.Parent.Absent)\n");
    let missing = fixture.run(&["check", "input.luau"]);

    let failure =
        check(&fixture, request(&fixture, &["input.luau"])).expect("unresolved mapped import");

    assert_eq!(missing.status, 1, "{}", missing.stderr);
    assert_eq!(failure.findings[0].category, DiagnosticCategory::Resolution);
    assert_eq!(failure.findings[0].code, "resolution.unresolved");
}

#[test]
fn modes() {
    let fixture = Fixture::empty();

    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=false\n",
    );

    fixture.write("input.luau", "local function identity(value)\n    return value\nend\nlocal number: number = identity(1)\nlocal string: string = identity('text')\nreturn number, string\n");
    let strict = fixture.run(&["check", "input.luau"]);
    assert_eq!(strict.status, 0, "{}", strict.stderr);

    fixture.write(
        "input.luau",
        "local function value(input)\n    return input.missing\nend\nreturn value(1)\n",
    );

    let strict = fixture.run(&["check", "input.luau"]);
    assert_eq!(strict.status, 1, "{}", strict.stderr);
    let nonstrict = fixture.run(&["check", "input.luau", "--mode", "nonstrict"]);
    assert_eq!(nonstrict.status, 0, "{}", nonstrict.stderr);

    fixture.write(
        "input.luau",
        "--!nocheck\nlocal value: string = 12\nreturn value\n",
    );

    let unchecked = fixture.run(&["check", "input.luau"]);
    assert_eq!(unchecked.status, 0, "{}", unchecked.stderr);
    let forced = fixture.run(&["check", "input.luau", "--mode", "strict"]);
    assert_eq!(forced.status, 1, "{}", forced.stderr);

    fixture.write(
        "input.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let directive = fixture.run(&["check", "input.luau", "--set", "checking.mode=\"nocheck\""]);

    assert_eq!(directive.status, 0, "{}", directive.stderr);

    fixture.write(
        "instar.toml",
        "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n",
    );

    let directive = fixture.run(&["check", "input.luau"]);
    assert_eq!(directive.status, 1, "{}", directive.stderr);

    fixture.write("input.luau", "--!nonstrict\nlocal function value(input)\n    return input.missing\nend\nreturn value(1)\n");
    assert_eq!(fixture.run(&["check", "input.luau"]).status, 0);
    fixture.write("input.luau", "--!nocheck\nlocal =\n");
    let malformed = fixture.run(&["check", "input.luau"]);

    let malformed_report =
        check(&fixture, request(&fixture, &["input.luau"])).expect("syntax report");

    assert_eq!(malformed.status, 1, "{}", malformed.stderr);

    assert!(
        malformed_report
            .findings
            .iter()
            .any(|finding| finding.category == DiagnosticCategory::Syntax)
    );

    fixture.write("input.luau", "return require('./dependency')\n");

    fixture.write(
        "dependency.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let dependency = fixture.run(&["check", "input.luau", "--mode", "nocheck"]);
    let mut invocation = request(&fixture, &["input.luau"]);
    invocation.overrides.checking.mode = Some(CheckingMode::Nocheck);
    let dependency_report = check(&fixture, invocation).expect("dependency mode findings");
    assert_eq!(dependency.status, 1, "{}", dependency.stderr);
    assert_eq!(dependency_report.counts.dependencies, 1);

    assert!(dependency_report.findings.iter().any(|finding| {
        finding.code == "1000"
            && dependency_report
                .snapshot
                .origin(finding.primary.source.identity)
                .and_then(|origin| origin.path())
                == Some(fixture.path("dependency.luau").as_path())
    }));

    assert_eq!(
        fixture.bytes("dependency.luau"),
        b"--!strict\nlocal value: string = 12\nreturn value\n"
    );
}

#[test]
fn operations() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: number = math.abs(-1) + 2\nreturn value\n",
    );

    let valid =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("valid operation report");

    assert_eq!(valid.completion, Completion::Complete);
    assert!(!valid.failed);

    for source in [
        "--!strict\nlocal value = 1 + 'wrong'\nreturn value\n",
        "--!strict\nlocal value = math.abs('wrong')\nreturn value\n",
    ] {
        fixture.write("probe.luau", source);

        let report =
            check(&fixture, request(&fixture, &["probe.luau"])).expect("operation findings");

        assert!(report.failed);
        assert_eq!(report.completion, Completion::Complete);
        assert_ne!(report.findings, []);

        assert!(report.findings.iter().all(|finding| {
            finding.category == DiagnosticCategory::Type
                && report
                    .snapshot
                    .origin(finding.primary.source.identity)
                    .and_then(|origin| origin.path())
                    == Some(fixture.path("probe.luau").as_path())
        }));

        assert!(report.findings.iter().any(|finding| {
            finding.message.contains("number") && finding.message.contains("string")
        }));
    }
}

#[test]
fn partial() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("broken/init.luau", "--!strict\nlocal =\nreturn 1\n");
    fixture.write("probe.luau", "--!strict\nlocal dependency = require('./broken')\nlocal value: string = 12\nreturn dependency, value\n");
    let output = fixture.run(&["check", "probe.luau"]);

    let report =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("partial analysis report");

    assert_ne!(output.status, 0, "{}", output.stderr);

    for (path, category) in [
        ("broken/init.luau", DiagnosticCategory::Syntax),
        ("probe.luau", DiagnosticCategory::Type),
    ] {
        assert!(report.findings.iter().any(|finding| {
            finding.category == category
                && report
                    .snapshot
                    .origin(finding.primary.source.identity)
                    .and_then(|origin| origin.path())
                    == Some(fixture.path(path).as_path())
        }));
    }

    let prerequisite = report
        .findings
        .iter()
        .find(|finding| finding.code == "analysis.dependency")
        .expect("dependency prerequisite");

    assert!(prerequisite.related.iter().any(|related| {
        report
            .snapshot
            .origin(related.location.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("broken/init.luau").as_path())
    }));

    snapshot(
        &fixture,
        "check",
        "partial",
        &json!({"findings": attributed(&report.findings, &report.snapshot), "sources": report.sources, "completion": report.completion}),
    );
}

#[test]
fn profiles() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[language.flags]\nLuauCyclicRequireTypeInference=true\n");
    fixture.write("input.luau", "return require('./nested/dependency')\n");
    fixture.write("nested/dependency.luau", "return {value = 1}\n");
    let compatible = fixture.run(&["check", "input.luau"]);

    let report =
        check(&fixture, request(&fixture, &["input.luau"])).expect("compatible language profiles");

    assert_eq!(compatible.status, 0, "{}", compatible.stderr);

    assert!(report.settings.iter().all(|effective| {
        effective
            .settings
            .language
            .flags
            .get("LuauCyclicRequireTypeInference")
            == Some(&FlagValue::Boolean(true))
    }));

    fixture.write(
        "nested/instar.toml",
        "[language.flags]\nLuauCyclicRequireTypeInference=false\n",
    );

    let incompatible = fixture.run(&["check", "input.luau"]);

    let failure =
        check(&fixture, request(&fixture, &["input.luau"])).expect("cross-profile report");

    assert_eq!(incompatible.status, 4, "{}", incompatible.stderr);
    assert_ne!(failure.completion, Completion::Complete);
    assert_eq!(failure.counts.dependencies, 1);

    assert_eq!(
        failure
            .findings
            .iter()
            .filter(|finding| finding.code == "analysis.cross_profile")
            .count(),
        2
    );

    for setting in [
        "language.flags.AbsentFlag=true",
        "language.flags.LuauCyclicRequireTypeInference=1",
        "language.flags.DebugLuauLogSolver=true",
        "language.flags.DebugLuauTypeFunctionRuntimeHeapLimit=0",
    ] {
        let rejected = fixture.run(&["check", "input.luau", "--set", setting]);
        assert_eq!(rejected.status, 2, "{setting}: {}", rejected.stderr);
        assert_ne!(rejected.stderr.trim(), "");
    }
}

#[test]
fn projects() {
    let fixture = Fixture::empty();

    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=false\n",
    );

    fixture.write("first/instar.toml", "[environment]\nglobals=['first']\n");
    fixture.write("second/instar.toml", "[environment]\nglobals=['second']\n");
    fixture.write("first/input.luau", "return first\n");
    fixture.write("second/input.lua", "return second\n");
    let output = fixture.run(&["check", "first", "second"]);

    let report =
        check(&fixture, request(&fixture, &["first", "second"])).expect("project settings");

    assert_eq!(output.status, 0, "{}", output.stderr);
    assert_eq!(report.counts.selected, 2);

    assert!(
        report
            .settings
            .iter()
            .any(|effective| effective.settings.environment.globals == ["first"])
    );

    assert!(
        report
            .settings
            .iter()
            .any(|effective| effective.settings.environment.globals == ["second"])
    );

    fixture.write("second/input.lua", "return first\n");
    let crossed = fixture.run(&["check", "first", "second"]);

    let diagnostics =
        check(&fixture, request(&fixture, &["first", "second"])).expect("crossed project findings");

    assert_eq!(crossed.status, 1, "{}", crossed.stderr);

    assert!(diagnostics.findings.iter().all(|finding| {
        diagnostics
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("second/input.lua").as_path())
    }));

    let explicit = fixture.run(&["check", "--project-root", "first"]);
    assert_eq!(explicit.status, 0, "{}", explicit.stderr);
    let mut invocation = request(&fixture, &[]);
    invocation.invocation.project_root = Some(fixture.path("first"));
    let explicit_report = check(&fixture, invocation).expect("explicit project root");
    assert_eq!(explicit_report.counts.selected, 1);
    let outside = fixture.run(&["check", "second/input.lua", "--project-root", "first"]);
    assert_eq!(outside.status, 2, "{}", outside.stderr);

    fixture.write(
        "alternate.toml",
        "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n",
    );

    let configured = fixture.run(&[
        "check",
        "second/input.lua",
        "--configuration",
        "alternate.toml",
    ]);

    assert_eq!(configured.status, 0, "{}", configured.stderr);
    let mut invocation = request(&fixture, &["second/input.lua"]);
    invocation.invocation.configuration = Some(fixture.path("alternate.toml"));
    let configured_report = check(&fixture, invocation).expect("explicit configuration");

    assert_eq!(
        configured_report.settings[0].settings.checking,
        CheckingMode::Nocheck
    );

    fixture.write("second/input.lua", "return second\n");
    let project = fixture.project();

    project
        .change(ChangeBatch {
            changes: vec![ProjectChange::WorkspaceRoots {
                added: vec![fixture.path("first"), fixture.path("second")],
                removed: vec![fixture.root().to_path_buf()],
            }],
        })
        .expect("register independent project contexts");

    let mut execution = options();
    execution.jobs = NonZeroUsize::MIN;

    let contexts = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![
                    Input::Path(fixture.path("first")),
                    Input::Path(fixture.path("second")),
                ],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        execution,
    ))
    .expect("multiple project analysis");

    complete(project.finish()).expect("finish multiple projects");
    assert_eq!(contexts.completion, Completion::Complete);
    assert!(!contexts.failed);
    assert_eq!(contexts.settings.len(), 2);
    assert_ne!(contexts.settings[0].context, contexts.settings[1].context);

    assert_ne!(
        contexts.settings[0].settings.environment.globals,
        contexts.settings[1].settings.environment.globals
    );

    snapshot(
        &fixture,
        "check",
        "projects",
        &json!({
            "modules": contexts.modules,
            "contexts": contexts.settings.iter().map(|effective| json!({
                "context": effective.context,
                "source": effective.source,
                "globals": effective.settings.environment.globals,
            })).collect::<Vec<_>>(),
            "completion": contexts.completion,
        }),
    );
}

#[test]
fn recovery() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nlocal =\nreturn value\n",
    );

    let bytes = fixture.bytes("probe.luau");
    let report = check(&fixture, request(&fixture, &["probe.luau"])).expect("syntax recovery");
    assert!(report.failed);

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.category == DiagnosticCategory::Syntax)
    );

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.category == DiagnosticCategory::Type && finding.code == "1000")
    );

    assert_eq!(fixture.bytes("probe.luau"), bytes);
}

#[test]
fn related() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\ntype Duplicate = number\ntype Duplicate = string\nreturn 1\n",
    );

    let report =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("duplicate type findings");

    assert!(report.failed);

    let finding = report
        .findings
        .iter()
        .find(|finding| finding.message.contains("Duplicate"))
        .expect("duplicate type diagnostic");

    assert_eq!(finding.related.len(), 1);

    assert_eq!(
        report
            .snapshot
            .origin(finding.related[0].location.source.identity)
            .and_then(|origin| origin.path()),
        Some(fixture.path("probe.luau").as_path())
    );

    assert_eq!(finding.related[0].message, "Previous type definition");
    assert_eq!(finding.related[0].location.source, finding.primary.source);
    assert!(finding.related[0].location.span.start < finding.primary.span.start);

    snapshot(
        &fixture,
        "check",
        "related",
        &json!({"findings": report.findings, "completion": report.completion}),
    );
}

#[test]
fn reporting() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let human = fixture.run(&["check", "probe.luau", "--color", "never"]);
    let text = format!("{}{}", human.stdout, human.stderr);
    assert_eq!(human.status, 1);
    assert_eq!(human.stdout, "");
    assert!(text.contains("1000"));
    assert!(text.contains("probe.luau:2:"));
    assert!(text.contains("local value: string = 12"));
    assert!(text.contains("string"));
    assert!(text.contains("number"));
    assert!(text.contains("Checking complete: 1 file"));
    assert!(!text.contains('\u{1b}'));

    let quiet = fixture.run(&["check", "probe.luau", "--quiet", "--color", "never"]);
    let quiet_text = format!("{}{}", quiet.stdout, quiet.stderr);
    assert_eq!(quiet.status, 1);
    assert_eq!(quiet.stdout, "");
    assert!(quiet_text.contains("1000"));
    assert!(quiet_text.contains("local value: string = 12"));
    assert!(!quiet_text.contains("Checking complete"));

    let colored = fixture.run(&["check", "probe.luau", "--color", "always"]);
    assert_eq!(colored.status, 1);
    assert!(format!("{}{}", colored.stdout, colored.stderr).contains("\u{1b}["));
    let automatic = fixture.run(&["check", "probe.luau", "--color", "auto"]);
    assert_eq!(automatic.status, 1);
    assert!(!format!("{}{}", automatic.stdout, automatic.stderr).contains('\u{1b}'));

    let verbose = fixture.run(&["check", "probe.luau", "--verbose", "--color", "never"]);
    assert_eq!(verbose.status, 1, "{}", verbose.stderr);
    assert_eq!(verbose.stdout, "");
    assert!(verbose.stderr.lines().any(|line| line.contains("DEBUG")));
}

fn request(fixture: &Fixture, paths: &[&str]) -> CheckRequest {
    CheckRequest {
        invocation: Invocation {
            inputs: paths
                .iter()
                .map(|path| Input::Path(fixture.path(path)))
                .collect(),
            ..Invocation::default()
        },
        ..CheckRequest::default()
    }
}

#[test]
fn reuse() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("probe.luau", "--!strict\nlocal library = require('./library')\nlocal value: number = library.double(2)\nreturn value\n");
    let project = fixture.project();
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let mut execution = options();
    execution.jobs = NonZeroUsize::MIN;

    execution.observer = Some(Arc::new(move |event| {
        if let EventPayload::Progress(progress) = event.payload {
            recorded.lock().expect("progress events").push(progress);
        }
    }));

    let request = CheckRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(fixture.path("probe.luau"))],
            ..Invocation::default()
        },
        ..CheckRequest::default()
    };

    let first =
        complete(project.check(request.clone(), execution.clone())).expect("first analysis");

    assert_eq!(first.completion, Completion::Complete);
    assert!(!first.failed);
    assert_eq!(first.counts.selected, 1);
    assert_eq!(first.counts.dependencies, 1);
    let measured = events.lock().expect("first progress").clone();

    assert!(measured.iter().any(|progress| progress.phase == "Checking"
        && progress.completed == 0
        && progress.total == Some(2)));

    assert!(measured.iter().any(|progress| progress.phase == "Checking"
        && progress.completed == 2
        && progress.total == Some(2)));

    events.lock().expect("clear events").clear();

    let second =
        complete(project.check(request.clone(), execution.clone())).expect("unchanged analysis");

    assert_eq!(second.completion, Completion::Complete);
    assert!(!second.failed);
    assert_eq!(second.sources, first.sources);
    assert_eq!(second.findings, first.findings);
    assert!(project.current(&first.inputs));

    assert!(
        !events
            .lock()
            .expect("reuse events")
            .iter()
            .any(|progress| progress.phase == "Checking")
    );

    let library = fixture.path("library.luau");

    let previous = first
        .snapshot
        .source_at(&library)
        .expect("previous library")
        .clone();

    fixture.write("library.luau", "local function double(value: number): string\n    return tostring(value * 2)\nend\nreturn {double = double}\n");

    project
        .change(ChangeBatch {
            changes: vec![ProjectChange::Invalidate {
                paths: vec![library.clone()],
            }],
        })
        .expect("invalidate dependency");

    assert!(!project.current(&first.inputs));
    events.lock().expect("clear events").clear();

    let changed = complete(project.check(request, execution)).expect("changed analysis");
    complete(project.finish()).expect("finish reused project");
    assert_eq!(changed.completion, Completion::Complete);
    assert!(changed.failed);

    let replacement = changed
        .snapshot
        .source_at(&library)
        .expect("replacement library");

    assert_eq!(replacement.version.identity, previous.version.identity);
    assert!(replacement.version.revision > previous.version.revision);
    assert_ne!(replacement.text, previous.text);

    assert!(
        changed
            .findings
            .iter()
            .any(|diagnostic| diagnostic.code == "1000"
                && diagnostic.category == DiagnosticCategory::Type)
    );

    assert!(
        events
            .lock()
            .expect("changed progress")
            .iter()
            .any(|progress| progress.phase == "Checking")
    );

    assert_eq!(
        first
            .snapshot
            .source_at(&library)
            .expect("retained original")
            .text,
        previous.text
    );

    snapshot(
        &fixture,
        "check",
        "reuse",
        &json!({
            "initial": {"sources": first.sources, "findings": attributed(&first.findings, &first.snapshot)},
            "unchanged": {"sources": second.sources, "findings": attributed(&second.findings, &second.snapshot)},
            "changed": {"sources": changed.sources, "findings": attributed(&changed.findings, &changed.snapshot)},
        }),
    );
}

#[test]
fn roblox() {
    let fixture = Fixture::new("project", "assets/project");

    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=true\nasset_directory='.'\n",
    );

    fixture.write("input.luau", "local part: Part = Instance.new('Part')\nlocal name: string = part.Name\nlocal world: DataModel = game\nreturn part, name, world\n");
    let supported = fixture.run(&["check", "input.luau"]);
    let report = check(&fixture, request(&fixture, &["input.luau"])).expect("Roblox environment");
    assert_eq!(supported.status, 0, "{}", supported.stderr);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.findings, 0);
    assert!(report.settings[0].settings.environment.roblox);
    fixture.write("sourcemap.json", "{\"name\":\"Place\",\"className\":\"DataModel\",\"children\":[{\"name\":\"Library\",\"className\":\"Folder\",\"children\":[{\"name\":\"Caller\",\"className\":\"ModuleScript\",\"filePaths\":[\"mapped.luau\"]},{\"name\":\"Target\",\"className\":\"ModuleScript\",\"filePaths\":[\"target.luau\"]}]}]}\n");
    fixture.write("mapped.luau", "local target = require(script.Parent.Target)\nlocal value: target.Value = target\nlocal result: number = value.value\nreturn result\n");

    fixture.write(
        "target.luau",
        "export type Value = {value: number}\nreturn {value = 1}\n",
    );

    let mapped = fixture.run(&["check", "mapped.luau"]);

    let mapping =
        check(&fixture, request(&fixture, &["mapped.luau"])).expect("typed mapped closure");

    assert_eq!(mapped.status, 0, "{}", mapped.stderr);
    assert_eq!(mapping.completion, Completion::Complete);
    assert_eq!(mapping.counts.dependencies, 1);
    assert_eq!(mapping.dependencies.len(), 1);
    assert!(mapping.imports[0].failure.is_none());
    fixture.write("input.luau", "local part: Part = Instance.new('Part')\npart.Name = 'forbidden'\nlocal world = Instance.new('DataModel')\nreturn part, world\n");
    let invalid = fixture.run(&["check", "input.luau"]);
    let failure = check(&fixture, request(&fixture, &["input.luau"])).expect("capability findings");
    assert_eq!(invalid.status, 1, "{}", invalid.stderr);

    assert!(
        failure
            .findings
            .iter()
            .any(|finding| finding.message.contains("Name") && finding.message.contains("read"))
    );

    assert!(
        failure
            .findings
            .iter()
            .any(|finding| finding.message == "Instance.new does not support class 'DataModel'")
    );

    fixture.write("plain/instar.toml", "[environment]\nroblox=false\n");

    fixture.write(
        "plain/input.luau",
        "local part: Part = Instance.new('Part')\nreturn part\n",
    );

    let selected = fixture.run(&["check", "plain/input.luau"]);

    let plain = check(&fixture, request(&fixture, &["plain/input.luau"]))
        .expect("plain environment findings");

    assert_eq!(selected.status, 1, "{}", selected.stderr);
    assert!(!plain.settings[0].settings.environment.roblox);

    assert!(
        plain
            .findings
            .iter()
            .any(|finding| finding.message.contains("Instance"))
    );

    snapshot(
        &fixture,
        "check",
        "roblox",
        &json!({
            "supported": {"counts": report.counts, "completion": report.completion},
            "capabilities": attributed(&failure.findings, &failure.snapshot),
            "plain": attributed(&plain.findings, &plain.snapshot),
            "mapping": {"dependencies": mapping.dependencies, "counts": mapping.counts, "completion": mapping.completion},
        }),
    );
}

#[test]
fn sandbox() {
    let fixture = Fixture::new("luau", "sandbox");
    fixture.write("input.luau", "local value: string = 12\nreturn value\n");

    let valid = fixture.run(&["check", "input.luau", "--set", "environment.roblox=false"]);
    let mut invocation = request(&fixture, &["input.luau"]);
    invocation.overrides.environment.roblox = Some(false);
    let report = check(&fixture, invocation).expect("sandboxed configuration");
    assert_eq!(valid.status, 1, "{}", valid.stderr);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.settings[0].settings.checking, CheckingMode::Strict);
    assert_eq!(report.findings[0].category, DiagnosticCategory::Type);

    fixture.write(
        "instar.luau",
        "os.execute('forbidden')\nreturn {checking = {mode = 'nocheck'}}\n",
    );

    let blocked = fixture.run(&["check", "input.luau"]);

    let failure = check(&fixture, request(&fixture, &["input.luau"]))
        .expect_err("sandbox rejects os.execute");

    assert_eq!(blocked.status, 2, "{}", blocked.stderr);
    assert_ne!(failure.completion, Completion::Complete);

    assert!(
        failure
            .diagnostics
            .iter()
            .any(|finding| finding.category == DiagnosticCategory::Configuration)
    );

    assert!(failure.message.contains("instar.luau"));
    fixture.write("instar.toml", "[environment]\nroblox=false\n");
    let competing = fixture.run(&["check", "input.luau"]);

    let competing_report =
        check(&fixture, request(&fixture, &["input.luau"])).expect_err("competing configuration");

    assert_eq!(competing.status, 2, "{}", competing.stderr);

    assert_eq!(
        competing_report
            .diagnostics
            .iter()
            .filter(|finding| finding.code == "configuration.competing")
            .count(),
        2
    );
}

#[test]
fn schema() {
    let schema = Project::configuration_schema();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);

    assert!(
        schema["properties"]
            .as_object()
            .expect("project properties")
            .contains_key("checking")
    );

    assert!(
        schema["properties"]
            .as_object()
            .expect("project properties")
            .contains_key("selection")
    );

    assert!(
        schema["properties"]
            .as_object()
            .expect("project properties")
            .contains_key("environment")
    );
}

#[test]
fn selection() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[selection.checking]\ninclude=['source/**']\nexclude=['source/excluded.luau']\n");
    fixture.write(".gitignore", "source/ignored.luau\n");
    fixture.write("source/selected.luau", "return 1\n");

    fixture.write(
        "source/ignored.luau",
        "local value: string = 12\nreturn value\n",
    );

    fixture.write(
        "source/excluded.luau",
        "local value: string = 12\nreturn value\n",
    );

    fixture.write("outside.luau", "local value: string = 12\nreturn value\n");
    let discovered = fixture.run(&["check", "--explain-selection"]);
    let report = check(&fixture, request(&fixture, &[])).expect("discovered selection");
    assert_eq!(discovered.status, 0, "{}", discovered.stderr);
    assert!(discovered.stderr.contains("Selection:"));
    assert_eq!(report.counts.selected, 1);
    assert_eq!(report.counts.excluded, 3);
    let decisions = &report.selection;

    for (path, reason) in [
        (
            "source/ignored.luau",
            "filesystem ignore: source/ignored.luau",
        ),
        ("source/excluded.luau", "configured exclusion"),
        ("outside.luau", "not matched by includes"),
    ] {
        let decision = decisions
            .iter()
            .find(|decision| decision.input == fixture.path(path).to_string_lossy())
            .expect("selection decision");

        assert!(!decision.selected);
        assert_eq!(decision.reason, reason);
        assert!(discovered.stderr.contains(reason));
    }

    let ignore = decisions
        .iter()
        .find(|decision| decision.input == fixture.path("source/ignored.luau").to_string_lossy())
        .expect("ignore decision");

    assert_eq!(ignore.origin, Some(fixture.path(".gitignore")));
    let explicit = fixture.run(&["check", "source/ignored.luau"]);
    assert_eq!(explicit.status, 1, "{}", explicit.stderr);

    let explicit_report = check(&fixture, request(&fixture, &["source/ignored.luau"]))
        .expect("explicit ignored source");

    assert_eq!(explicit_report.counts.selected, 1);
    let excluded = fixture.run(&["check", "source/excluded.luau"]);

    let empty =
        check(&fixture, request(&fixture, &["source/excluded.luau"])).expect("explicit exclusion");

    assert_eq!(excluded.status, 0, "{}", excluded.stderr);
    assert_eq!(empty.counts.selected, 0);
    assert_eq!(empty.counts.excluded, 1);
    assert_eq!(empty.selection[0].reason, "configured exclusion");

    let override_selection =
        fixture.run(&["check", "source/excluded.luau", "--override-exclusions"]);

    assert_eq!(
        override_selection.status, 1,
        "{}",
        override_selection.stderr
    );

    let mut invocation = request(&fixture, &["source/excluded.luau"]);
    invocation.overrides.selection.override_exclusions = true;
    let overridden = check(&fixture, invocation).expect("selection override");
    assert_eq!(overridden.counts.selected, 1);

    let command = fixture.run(&[
        "check",
        "--include",
        "outside.luau",
        "--exclude",
        "source/**",
    ]);

    assert_eq!(command.status, 1, "{}", command.stderr);
    let mut invocation = request(&fixture, &[]);
    invocation.overrides.selection.include = Some(vec!["outside.luau".to_owned()]);
    invocation.overrides.selection.exclude = Some(vec!["source/**".to_owned()]);
    let command_report = check(&fixture, invocation).expect("command selection");
    assert_eq!(command_report.counts.selected, 1);

    assert_eq!(
        command_report
            .snapshot
            .origin(command_report.findings[0].primary.source.identity)
            .and_then(|origin| origin.path()),
        Some(fixture.path("outside.luau").as_path())
    );

    snapshot(
        &fixture,
        "check",
        "selection",
        &json!({
            "selection": decisions, "counts": report.counts,
            "empty": {"selection": empty.selection, "counts": empty.counts},
        }),
    );
}

#[test]
fn stdin() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("nested/input.luau", "return 'disk'\n");
    let disk = fixture.bytes("nested/input.luau");
    let text = "--!strict\nlocal library = require('../library')\nlocal value: string = library.double(2)\nreturn value\n";

    let output = fixture.stdin(
        &[
            "check",
            "-",
            "consumer.lua",
            "--stdin-filepath",
            "nested/input.luau",
        ],
        text,
    );

    let mut invocation = request(&fixture, &["consumer.lua"]);

    invocation.invocation.inputs.insert(
        0,
        Input::Stdin {
            text: Arc::from(text),
            context: Some(fixture.path("nested/input.luau")),
        },
    );

    let report = check(&fixture, invocation).expect("stdin analysis");
    assert_eq!(output.status, 1, "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert_eq!(report.counts.selected, 2);
    assert_eq!(report.counts.dependencies, 2);

    let finding = report
        .findings
        .iter()
        .find(|finding| {
            report
                .snapshot
                .origin(finding.primary.source.identity)
                .and_then(|origin| origin.path())
                == Some(fixture.path("nested/input.luau").as_path())
        })
        .expect("stdin identity");

    assert_eq!(finding.category, DiagnosticCategory::Type);
    assert_eq!(finding.code, "1000");
    assert!(finding.primary.source.revision > 0);
    let start = finding.primary.span.start;
    let end = finding.primary.span.end;
    assert_eq!(&text[start..end], "library.double(2)");
    assert_eq!(fixture.bytes("nested/input.luau"), disk);

    let plain = fixture.stdin(
        &["check", "-", "--mode", "strict"],
        "local value: string = 12\nreturn value\n",
    );

    assert_eq!(plain.status, 1, "{}", plain.stderr);
    assert!(plain.stderr.contains("stdin.luau:1:"));
    let mut invocation = request(&fixture, &[]);

    invocation.invocation.inputs = vec![Input::Stdin {
        text: Arc::from("local value: string = 12\nreturn value\n"),
        context: None,
    }];

    invocation.overrides.checking.mode = Some(CheckingMode::Strict);
    let plain_report = check(&fixture, invocation).expect("contextless stdin");

    assert!(matches!(
        plain_report
            .snapshot
            .origin(plain_report.findings[0].primary.source.identity),
        Some(SourceOrigin::Stdin { context: None })
    ));

    snapshot(
        &fixture,
        "check",
        "stdin",
        &json!({
            "findings": attributed(&report.findings, &report.snapshot), "selection": report.selection,
            "dependencies": report.dependencies, "counts": report.counts, "completion": report.completion,
        }),
    );
}

#[test]
fn streaming() {
    let fixture = Fixture::new("cli", "project");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );

    let output = fixture.run(&["check", "probe.luau", "--streaming", "--color", "never"]);
    assert_eq!(output.status, 1, "{}", output.stderr);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr.matches("error[1000]").count(), 1);
    assert!(output.stderr.contains("probe.luau:2:"));
    assert!(output.stderr.contains("local value: string = 12"));
    let finding_position = output.stderr.find("error[1000]").expect("streamed finding");

    let summary_position = output
        .stderr
        .find("Checking complete: 1 file")
        .expect("terminal summary");

    assert!(finding_position < summary_position);
    let project = fixture.project();
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    let mut execution = options();

    execution.observer = Some(Arc::new(move |event| {
        if let EventPayload::Diagnostic(finding) = event.payload {
            recorded.lock().expect("stream findings").push(finding);
        }
    }));

    let report = complete(project.check(request(&fixture, &["probe.luau"]), execution))
        .expect("streamed check");

    complete(project.finish()).expect("finish streaming project");
    let findings = events.lock().expect("recorded findings");
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].code, "1000");

    assert_eq!(
        report
            .snapshot
            .origin(findings[0].primary.source.identity)
            .and_then(|origin| origin.path()),
        Some(fixture.path("probe.luau").as_path())
    );

    assert_eq!(report.completion, Completion::Complete);
    assert!(report.failed);
    assert_eq!(report.findings[0], findings[0]);

    snapshot(
        &fixture,
        "check",
        "streaming",
        &json!({
            "findings": &*findings,
            "terminal": {"completion": report.completion, "failed": report.failed, "findings": report.findings},
        }),
    );
}

#[test]
fn timeout() {
    let fixture = Fixture::new("luau", "timeout");
    fixture.write("input.luau", "return 1\n");
    let configuration = fixture.bytes("instar.luau");
    let output = fixture.run(&["check", "input.luau", "--timeout", "1"]);
    assert_eq!(output.status, 4, "{}", output.stderr);
    assert_ne!(output.stderr.trim(), "");
    let project = fixture.project();
    let execution = Options::new(Some(Instant::now() + Duration::from_secs(1)));

    let failure = complete(project.check(request(&fixture, &["input.luau"]), execution))
        .expect_err("configuration deadline");

    complete(project.finish()).expect("finish timed out project");
    assert_eq!(failure.completion, Completion::TimedOut);
    assert_ne!(failure.message, "");
    assert_eq!(fixture.bytes("instar.luau"), configuration);
}

#[test]
fn typefunctions() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("probe.luau", "--!strict\ntype function identity(value)\n    return value\nend\nlocal value: identity<number> = 12\nlocal number: number = value\nreturn number\n");

    let valid =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("valid type function report");

    assert_eq!(valid.completion, Completion::Complete);
    assert!(!valid.failed);
    fixture.write("probe.luau", "--!strict\ntype function identity(value)\n    return value\nend\nlocal value: identity<number> = 'wrong'\nreturn value\n");

    let report =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("type function findings");

    assert_eq!(report.completion, Completion::Complete);
    assert!(report.failed);

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "1000" && finding.message.contains("number"))
    );

    fixture.write("probe.luau", "--!strict\ntype function forbidden(value)\n    gcinfo()\n    return value\nend\nlocal value: forbidden<number> = 12\nreturn value\n");

    let failure = check(&fixture, request(&fixture, &["probe.luau"]))
        .expect("type function sandbox findings");

    assert_eq!(failure.completion, Completion::Complete);
    assert!(failure.failed);

    assert!(
        failure
            .findings
            .iter()
            .any(|finding| finding.message.contains("not supported in type functions"))
    );

    fixture.write("probe.luau", "--!strict\ntype function exhausted()\n    local value = string.rep('x', 128 * 1024 * 1024)\n    return types.singleton(value)\nend\nlocal value: exhausted<> = 'text'\nreturn value\n");

    let incomplete =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("type function resource report");

    assert_ne!(incomplete.completion, Completion::Complete);

    assert!(
        incomplete
            .findings
            .iter()
            .any(|finding| finding.message.contains("memory")
                && finding.message.contains("type function"))
    );
}

#[test]
fn unicode() {
    let fixture = Fixture::new("cli", "diagnostics");

    fixture.write(
        "instar.toml",
        "[environment]\nroblox=false\n[resolution]\nsourcemaps=[]\n",
    );

    let text = fixture.read("unicode.luau");
    let bytes = fixture.bytes("unicode.luau");

    let report =
        check(&fixture, request(&fixture, &["unicode.luau"])).expect("Unicode location report");

    assert!(report.failed);

    let finding = report
        .findings
        .iter()
        .find(|finding| finding.code == "1000")
        .expect("type mismatch");

    let start = finding.primary.span.start;
    let end = finding.primary.span.end;
    assert_eq!(&text[start..end], "12");
    assert_eq!(start, text.find("12").expect("literal byte offset"));

    assert_eq!(
        report
            .snapshot
            .origin(finding.primary.source.identity)
            .and_then(|origin| origin.path()),
        Some(fixture.path("unicode.luau").as_path())
    );

    assert_eq!(finding.primary.source, finding.identity.location().source);

    assert_eq!(
        finding.primary.source.revision,
        report
            .snapshot
            .source(finding.primary.source.identity)
            .expect("diagnostic source")
            .version
            .revision
    );

    assert_eq!(fixture.bytes("unicode.luau"), bytes);

    snapshot(
        &fixture,
        "check",
        "unicode",
        &json!({"findings": report.findings, "sources": report.sources, "completion": report.completion}),
    );
}

#[test]
fn unresolved() {
    let fixture = Fixture::new("cli", "project");
    let text = "--!strict\nlocal dependency = require('./absent')\nlocal value: string = 12\nreturn dependency, value\n";
    fixture.write("probe.luau", text);

    let report =
        check(&fixture, request(&fixture, &["probe.luau"])).expect("unresolved import findings");

    assert!(report.failed);

    let unresolved = report
        .findings
        .iter()
        .find(|finding| finding.code == "resolution.unresolved")
        .expect("unresolved import");

    assert_eq!(unresolved.category, DiagnosticCategory::Resolution);
    let start = unresolved.primary.span.start;
    let end = unresolved.primary.span.end;
    assert_eq!(&text[start..end], "'./absent'");

    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.code == "1000" && finding.category == DiagnosticCategory::Type)
    );

    assert_eq!(report.counts.dependencies, 0);
    assert_eq!(report.imports[0].outcome, ImportOutcome::Unresolved);

    assert_ne!(
        report.imports[0]
            .failure
            .as_ref()
            .expect("resolution failure")
            .message,
        ""
    );
}

#[test]
fn watch() {
    for (name, path, initial, replacement, failed_before, failed_after) in [
        (
            "mutation",
            "dependency.luau",
            "return {value = 1}\n",
            "return {value = 'changed'}\n",
            false,
            true,
        ),
        (
            "policy",
            "instar.toml",
            "[checking]\nmode='strict'\n[environment]\nroblox=false\n[[environment.groups]]\nnamespace='@watch'\ndefinitions=['types.luau']\n",
            "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n[[environment.groups]]\nnamespace='@watch'\ndefinitions=['types.luau']\n",
            true,
            false,
        ),
        (
            "declaration",
            "types.luau",
            "declare supplied: number\n",
            "declare supplied: string\n",
            false,
            true,
        ),
    ] {
        let fixture = Fixture::empty();
        fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[[environment.groups]]\nnamespace='@watch'\ndefinitions=['types.luau']\n");
        fixture.write("types.luau", "declare supplied: number\n");
        fixture.write("dependency.luau", "return {value = 1}\n");

        fixture.write("input.luau", if name == "policy" {
            "local value: string = 12\nreturn value\n"
        } else {
            "local dependency = require('./dependency')\nlocal value: number = dependency.value\nlocal global: number = supplied\nreturn value, global\n"
        });

        fixture.write(path, initial);
        let project = fixture.project();
        let mut execution = options();
        execution.jobs = NonZeroUsize::MIN;

        let request = WatchRequest {
            operation: WatchOperation::Check(CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(fixture.path("input.luau"))],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            }),
            poll_interval: Duration::from_millis(10),
        };

        let first =
            complete(project.watch(request.clone(), execution.clone())).expect("initial watch");

        assert_eq!(first.completion, Completion::Complete);
        assert_eq!(first.iterations.len(), 1);

        let WatchCommandReport::Check(original) = &first.iterations[0].report else {
            panic!("check watch returned another operation");
        };

        assert_eq!(original.failed, failed_before, "{name}");
        let changed_path = fixture.path(path);

        let previous = original
            .snapshot
            .source_at(&changed_path)
            .expect("consulted watched input")
            .clone();

        fixture.write(path, replacement);

        let second = complete(project.watch(request, execution)).expect("changed watch");
        complete(project.finish()).expect("finish watched project");
        assert_eq!(second.completion, Completion::Complete);
        assert_eq!(second.iterations.len(), 1);

        assert!(
            second.iterations[0]
                .changed
                .iter()
                .any(|input| input.identity == InputIdentity::Path(changed_path.clone())),
            "{name}: {:?}",
            second.iterations[0].changed
        );

        let WatchCommandReport::Check(updated) = &second.iterations[0].report else {
            panic!("check watch returned another operation");
        };

        assert_eq!(updated.failed, failed_after, "{name}");

        let next = updated
            .snapshot
            .source_at(&changed_path)
            .expect("changed input");

        assert_eq!(next.version.identity, previous.version.identity);
        assert!(next.version.revision > previous.version.revision);
        assert_eq!(previous.text.as_ref(), initial);
        assert_eq!(next.text.as_ref(), replacement);
        assert!(project.current(&updated.inputs));
        assert!(!project.current(&original.inputs));

        if failed_after {
            assert!(
                updated
                    .findings
                    .iter()
                    .any(|diagnostic| diagnostic.code == "1000"
                        && diagnostic.category == DiagnosticCategory::Type),
                "{name}: {:?}",
                updated.findings
            );
        }

        snapshot(
            &fixture,
            "check",
            name,
            &json!({
                "case": name,
                "original": {"findings": attributed(&original.findings, &original.snapshot), "sources": original.sources, "failed": original.failed},
                "updated": {"findings": attributed(&updated.findings, &updated.snapshot), "sources": updated.sources, "failed": updated.failed, "changed": second.iterations[0].changed},
            }),
        );
    }
}
