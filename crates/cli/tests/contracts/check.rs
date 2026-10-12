use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
    time::Duration,
};

use instar_project::{
    ChangeBatch, CheckRequest, Input, InputIdentity, Invocation, Project, ProjectChange,
    WatchCommandReport, WatchOperation, WatchRequest,
};
use instar_shared::{Completion, DiagnosticCategory, EventPayload};
use serde_json::{Value, json};

use super::support::{Fixture, complete, diagnostic, options, snapshot};

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
            "diagnostics": failure.diagnostics,
            "completed_sources": failure.completed_sources,
        }),
    );
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
        "--json",
        "--explain-dependencies",
        "--jobs",
        "1",
    ]);
    let report = output.json();

    assert_eq!(output.status, 0, "{report}");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["dependencies"], 2);
    assert_eq!(report["counts"]["findings"], 0);
    assert_eq!(report["sources"].as_array().expect("sources").len(), 3);
    assert_eq!(report["diagnostics"], json!([]));
    assert_eq!(
        report["dependencies"]
            .as_array()
            .expect("dependencies")
            .len(),
        3
    );

    let parallel = fixture.run(&["check", "consumer.lua", "--json", "--jobs", "2"]);
    let parallel_report = parallel.json();
    assert_eq!(parallel.status, 0, "{parallel_report}");
    assert_eq!(parallel_report["counts"], report["counts"]);
    assert_eq!(parallel_report["diagnostics"], report["diagnostics"]);
    assert_eq!(parallel_report["dependencies"], report["dependencies"]);
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
            "sources": report["sources"],
            "modules": report["modules"],
            "dependencies": report["dependencies"],
            "counts": report["counts"],
            "completion": report["completion"],
            "status": report["status"],
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
    let output = fixture.run(&["check", "nested/input.luau", "--json"]);
    let report = output.json();
    let effective = &report["settings"][0];

    assert_eq!(output.status, 0, "{report}");
    assert_eq!(effective["settings"]["checking"], "nocheck");
    assert_eq!(
        effective["settings"]["environment"]["globals"],
        json!(["child"])
    );

    let overridden = fixture.run(&["check", "nested/input.luau", "--json", "--mode", "strict"]);
    let strict = overridden.json();
    assert_eq!(overridden.status, 1, "{strict}");
    assert_eq!(strict["settings"][0]["settings"]["checking"], "strict");
    assert_eq!(strict["diagnostics"][0]["category"], "Type");
    assert_eq!(fixture.bytes("instar.toml"), configuration);
    assert_eq!(fixture.bytes("nested/instar.toml"), child);

    snapshot(
        &fixture,
        "check",
        "configuration",
        &json!({
            "inherited": {
                "mode": effective["settings"]["checking"],
                "globals": effective["settings"]["environment"]["globals"],
            },
            "override": {"mode": strict["settings"][0]["settings"]["checking"], "diagnostics": strict["diagnostics"]},
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
    let valid = fixture.run(&["check", "input.luau", "--json"]);
    let report = valid.json();

    assert_eq!(valid.status, 0, "{report}");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["dependencies"], 2);
    assert_eq!(
        report["dependencies"]
            .as_array()
            .expect("cycle edges")
            .len(),
        3
    );

    fixture.write(
        "first.luau",
        "local second = require('./second')\nreturn second\n",
    );
    fixture.write(
        "second.luau",
        "local first = require('./first')\nreturn first\n",
    );
    let invalid = fixture.run(&["check", "first.luau", "--json", "--explain-dependencies"]);
    let failed = invalid.json();
    assert_eq!(invalid.status, 1, "{failed}");
    let cycle = failed["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "1024")
        .expect("uninferrable cycle diagnostic");
    let related = cycle["related"].as_array().expect("cycle locations");
    assert!(
        related
            .iter()
            .any(|location| location["path"] == json!(fixture.path("first.luau")))
    );
    assert!(
        related
            .iter()
            .any(|location| location["path"] == json!(fixture.path("second.luau")))
    );
    assert!(related.iter().all(|location| {
        location["message"]
            .as_str()
            .is_some_and(|message| message.contains("requires"))
    }));
}

#[test]
fn declarations() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("probe.luau", "--!strict\nlocal inherited: number = subject.value\nlocal declared: string = subject.label\nlocal builtin: number = math.abs(-3)\nlocal length: number = string.len(declared)\nreturn inherited + builtin + length\n");
    let valid = fixture.run(&["check", "probe.luau", "--json"]);
    let report = valid.json();
    assert_eq!(valid.status, 0, "{report}");
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["findings"], 0);
    assert_eq!(report["completion"], "Complete");

    fixture.write(
        "probe.luau",
        "--!strict\nlocal inherited: string = subject.value\nreturn inherited\n",
    );
    let invalid = fixture.run(&["check", "probe.luau", "--json"]);
    let mismatch = invalid.json();
    assert_eq!(invalid.status, 1, "{mismatch}");
    assert_eq!(mismatch["diagnostics"][0]["category"], "Type");
    assert!(
        mismatch["diagnostics"][0]["message"]
            .as_str()
            .expect("message")
            .contains("number")
    );

    fixture.write("probe.luau", "--!strict\nreturn provided\n");
    let globals = fixture.run(&[
        "check",
        "probe.luau",
        "--json",
        "--set",
        "environment.globals=[\"provided\"]",
    ]);
    assert_eq!(globals.status, 0, "{}", globals.json());
    let unknown = fixture.run(&[
        "check",
        "probe.luau",
        "--json",
        "--set",
        "environment.globals=[]",
    ]);
    let unknown_report = unknown.json();
    assert_eq!(unknown.status, 1, "{unknown_report}");
    assert!(
        unknown_report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("provided")))
    );
}

#[test]
fn dependencies() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[selection.checking]\nexclude=['library/**']\n[resolution.aliases]\npackage='library'\n");
    fixture.write("library/init.luau", "export type Record = {value: number}\nlocal function double(value: number): number\n    return value * 2\nend\nreturn {double = double}\n");
    fixture.write("nested/input.luau", "local library = require('@package')\nlocal relative = require('../library')\nlocal record: library.Record = {value = relative.double(2)}\nlocal value: number = record.value\nreturn value\n");
    let clean = fixture.run(&[
        "check",
        "nested/input.luau",
        "--json",
        "--explain-dependencies",
    ]);
    let report = clean.json();
    assert_eq!(clean.status, 0, "{report}");
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["dependencies"], 1);
    assert_eq!(report["dependencies"].as_array().expect("edges").len(), 2);
    assert_eq!(report["imports"].as_array().expect("imports").len(), 2);
    assert!(
        report["imports"]
            .as_array()
            .expect("imports")
            .iter()
            .all(|import| import["failure"].is_null())
    );
    assert_eq!(
        report["dependencies"][0]["target"],
        report["dependencies"][1]["target"]
    );

    fixture.write("nested/input.luau", "local library = require('@package')\nlocal record: library.Record = {value = 'wrong'}\nreturn library.double('wrong'), record\n");
    fixture.write("library/init.luau", "export type Record = {value: number}\nlocal value: string = 12\nlocal function double(value: number): number\n    return value * 2\nend\nreturn {double = double, value = value}\n");
    let invalid = fixture.run(&["check", "nested/input.luau", "--json"]);
    let findings = invalid.json();
    assert_eq!(invalid.status, 1, "{findings}");
    assert_eq!(findings["counts"]["dependencies"], 1);
    for path in ["nested/input.luau", "library/init.luau"] {
        assert!(
            findings["diagnostics"]
                .as_array()
                .expect("diagnostics")
                .iter()
                .any(|diagnostic| diagnostic["path"] == json!(fixture.path(path))
                    && diagnostic["category"] == "Type"),
            "missing type findings in {path}: {findings}"
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
    let output = fixture.run(&["check", "probe.luau", "--json", "--output", "report.json"]);
    let report: Value = serde_json::from_str(&fixture.read("report.json")).expect("report file");
    assert_eq!(output.status, 1);
    assert_eq!(output.stdout, "");
    assert_eq!(output.stderr, "");
    assert_eq!(report["status"], 1);
    assert_eq!(report["diagnostics"][0]["category"], "Type");

    for path in ["probe.luau", "instar.toml", "types.luau"] {
        let bytes = fixture.bytes(path);
        let refused = fixture.run(&["check", "probe.luau", "--json", "--output", path]);
        assert_eq!(
            refused.status, 2,
            "input report destination {path}: {}",
            refused.stderr
        );
        assert!(
            refused.json()["error"]
                .as_str()
                .expect("error")
                .contains("input")
        );
        assert_eq!(fixture.bytes(path), bytes, "report clobbered {path}");
    }

    let failed = fixture.run(&[
        "check",
        "probe.luau",
        "--json",
        "--output",
        "absent/report.json",
    ]);
    let failure = failed.json();
    assert_eq!(failed.status, 3, "{failure}");
    assert_ne!(failure["completion"], "Complete");
    assert!(!fixture.path("absent/report.json").exists());
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
        "--json",
        "--set",
        "environment.groups=[{\"namespace\":\"@missing\",\"definitions\":[\"absent.luau\"]}]",
    ]);
    let report = missing.json();
    assert_eq!(missing.status, 4, "{report}");
    assert_ne!(report["completion"], "Complete");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "environment.unavailable")
    );

    fixture.write("broken.luau", "declare broken: MissingType\n");
    let invalid = fixture.run(&[
        "check",
        "probe.luau",
        "--json",
        "--set",
        "environment.groups=[{\"namespace\":\"@broken\",\"definitions\":[\"broken.luau\"]}]",
    ]);
    let failure = invalid.json();
    assert_eq!(invalid.status, 4, "{failure}");
    assert_ne!(failure["completion"], "Complete");
    assert!(
        failure["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["path"] == json!(fixture.path("broken.luau")))
    );
    let unavailable = failure["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "environment.unavailable")
        .expect("unavailable environment");
    assert!(
        unavailable["related"]
            .as_array()
            .expect("related locations")
            .iter()
            .any(|related| related["path"] == json!(fixture.path("broken.luau")))
    );

    let roblox = fixture.run(&[
        "check",
        "probe.luau",
        "--json",
        "--set",
        "environment.roblox=true",
        "--set",
        "environment.asset_directory=\"absent\"",
    ]);
    let unavailable = roblox.json();
    assert_eq!(roblox.status, 4, "{unavailable}");
    assert_ne!(unavailable["completion"], "Complete");
    assert!(
        unavailable["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "environment.unavailable")
    );

    snapshot(
        &fixture,
        "check",
        "environments",
        &json!({
            "missing": {"diagnostics": report["diagnostics"], "completion": report["completion"]},
            "invalid": {"diagnostics": failure["diagnostics"], "completion": failure["completion"]},
            "roblox": {"diagnostics": unavailable["diagnostics"], "completion": unavailable["completion"]},
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
    let complete = fixture.run(&["check", "probe.luau", "--json"]);
    let report = complete.json();
    assert_eq!(complete.status, 1, "{report}");
    assert_eq!(
        report["diagnostics"].as_array().expect("diagnostics").len(),
        1
    );
    assert_eq!(report["diagnostics"][0]["code"], "1000");
    assert_eq!(report["diagnostics"][0]["category"], "Type");
    assert_eq!(report["diagnostics"][0]["severity"], "error");
    assert_eq!(report["diagnostics"][0]["identity"]["code"], "1000");
    assert_eq!(report["diagnostics"][0]["identity"]["producer"], "luau");

    diagnostic(&report, "1000", "probe.luau", [32, 34]);
    let mut cases = Vec::new();
    for arguments in [
        ["--diagnostic-code", "absent"],
        ["--diagnostic-category", "syntax"],
        ["--diagnostic-severity", "warning"],
    ] {
        let output = fixture.run(&["check", "probe.luau", "--json", arguments[0], arguments[1]]);
        let filtered = output.json();
        assert_eq!(output.status, 1, "{filtered}");
        assert_eq!(filtered["status"], 1);
        assert_eq!(filtered["failed"], true);
        assert_eq!(filtered["completion"], "Complete");
        assert_eq!(filtered["sources"], report["sources"]);
        assert_eq!(filtered["modules"], report["modules"]);
        assert_eq!(filtered["diagnostics"], json!([]));
        cases.push(json!({"filter": arguments, "counts": filtered["counts"], "failed": filtered["failed"], "completion": filtered["completion"], "status": filtered["status"]}));
    }

    let included = fixture.run(&["check", "probe.luau", "--json", "--diagnostic-code", "1000"]);
    assert_eq!(included.status, 1);
    assert_eq!(included.json()["diagnostics"], report["diagnostics"]);
    snapshot(
        &fixture,
        "check",
        "filtering",
        &json!({"diagnostics": report["diagnostics"], "filtered": cases}),
    );
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

    let ordered = fixture.run(&["check", "input.luau", "--json"]);
    let report = ordered.json();
    assert_eq!(ordered.status, 0, "{report}");
    assert_eq!(
        report["settings"][0]["settings"]["environment"]["groups"][0]["namespace"],
        "@base"
    );
    assert_eq!(
        report["settings"][0]["settings"]["environment"]["groups"][1]["namespace"],
        "@child"
    );

    let reversed = fixture.run(&[
        "check", "input.luau", "--json", "--set",
        "environment.groups=[{\"namespace\":\"@child\",\"definitions\":[\"child.luau\"]},{\"namespace\":\"@base\",\"definitions\":[\"base.luau\"]}]",
    ]);
    let unavailable = reversed.json();
    assert_eq!(reversed.status, 4, "{unavailable}");
    assert_ne!(unavailable["completion"], "Complete");
    assert!(
        unavailable["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["path"] == json!(fixture.path("child.luau")))
    );
    fixture.write(
        "child.json",
        "{\"@base/global/subject\":{\"documentation\":\"Wrong namespace\"}}\n",
    );
    let mismatched = fixture.run(&["check", "input.luau", "--json"]);
    let attribution = mismatched.json();
    assert_eq!(mismatched.status, 4, "{attribution}");
    assert_ne!(attribution["completion"], "Complete");
    assert!(
        attribution["diagnostics"]
            .as_array()
            .expect("documentation diagnostics")
            .iter()
            .any(|finding| finding["path"] == json!(fixture.path("child.json")))
    );
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
        ("default", vec![], 3),
    ] {
        let mut command = vec!["check", "--json", "--explain-selection"];
        command.extend(arguments);
        let output = fixture.run(&command);
        let report = output.json();
        assert_eq!(output.status, 0, "{name}: {report}");
        assert_eq!(report["completion"], "Complete");
        assert_eq!(report["counts"]["selected"], selected, "{name}");
        assert_eq!(report["counts"]["dependencies"], 0);
        assert_eq!(report["counts"]["findings"], 0);
        cases.push(json!({"case": name, "selection": report["selection"], "counts": report["counts"], "completion": report["completion"]}));
    }
    snapshot(&fixture, "check", "inputs", &json!(cases));
}

#[test]
fn invalid() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("binary.luau", [0xFF, 0xFE]);
    fixture.write("text.txt", "return 1\n");
    fixture.write("broken.toml", "[checking]\nmode='impossible'\n");
    let mut failures = Vec::new();
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
        let mut command = vec!["check", "--json"];
        command.extend(arguments.iter().copied());
        let output = fixture.run(&command);
        let failure = output.json();
        assert_eq!(output.status, 2, "{arguments:?}: {failure}");
        assert_eq!(failure["status"], 2);
        assert!(
            failure["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty())
        );
        failures.push(json!({"arguments": arguments, "status": failure["status"], "completion": failure["completion"], "diagnostics": failure["diagnostics"]}));
    }
    let empty = Fixture::empty();
    let output = empty.run(&["check", "--json"]);
    assert_eq!(output.status, 2);
    assert!(
        output.json()["error"]
            .as_str()
            .expect("empty error")
            .contains("empty")
    );
    let duplicated = fixture.stdin(&["check", "-", "-", "--json"], "return 1\n");
    assert_eq!(duplicated.status, 2);
    assert!(
        duplicated.json()["error"]
            .as_str()
            .expect("stdin error")
            .contains("once")
    );
    snapshot(&fixture, "check", "invalid", &json!(failures));
}

#[test]
fn mapping() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n[resolution]\nsourcemaps=['sourcemap.json']\n");
    fixture.write("sourcemap.json", "{\"name\":\"Place\",\"className\":\"DataModel\",\"children\":[{\"name\":\"Library\",\"className\":\"Folder\",\"children\":[{\"name\":\"Caller\",\"className\":\"ModuleScript\",\"filePaths\":[\"input.luau\"]},{\"name\":\"Target\",\"className\":\"ModuleScript\",\"filePaths\":[\"target.luau\"]}]}]}\n");
    fixture.write("input.luau", "return require(script.Parent.Target)\n");
    fixture.write("target.luau", "return {value = 1}\n");
    let output = fixture.run(&["check", "input.luau", "--json", "--explain-dependencies"]);
    let report = output.json();
    assert_eq!(output.status, 0, "{report}");
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["dependencies"], 1);
    assert_eq!(
        report["dependencies"]
            .as_array()
            .expect("mapped dependency")
            .len(),
        1
    );
    assert!(report["imports"][0]["failure"].is_null());

    fixture.write("input.luau", "return require(script.Parent.Absent)\n");
    let missing = fixture.run(&["check", "input.luau", "--json"]);
    let failure = missing.json();
    assert_eq!(missing.status, 1, "{failure}");
    assert_eq!(failure["diagnostics"][0]["category"], "Resolution");
    assert_eq!(failure["diagnostics"][0]["code"], "resolution.unresolved");
}

#[test]
fn modes() {
    let fixture = Fixture::empty();
    fixture.write(
        "instar.toml",
        "[checking]\nmode='strict'\n[environment]\nroblox=false\n",
    );
    fixture.write("input.luau", "local function identity(value)\n    return value\nend\nlocal number: number = identity(1)\nlocal string: string = identity('text')\nreturn number, string\n");
    let strict = fixture.run(&["check", "input.luau", "--json"]);
    assert_eq!(strict.status, 0, "{}", strict.json());

    fixture.write(
        "input.luau",
        "local function value(input)\n    return input.missing\nend\nreturn value(1)\n",
    );
    let strict = fixture.run(&["check", "input.luau", "--json"]);
    assert_eq!(strict.status, 1, "{}", strict.json());
    let nonstrict = fixture.run(&["check", "input.luau", "--json", "--mode", "nonstrict"]);
    assert_eq!(nonstrict.status, 0, "{}", nonstrict.json());

    fixture.write(
        "input.luau",
        "--!nocheck\nlocal value: string = 12\nreturn value\n",
    );
    let unchecked = fixture.run(&["check", "input.luau", "--json"]);
    assert_eq!(unchecked.status, 0, "{}", unchecked.json());
    let forced = fixture.run(&["check", "input.luau", "--json", "--mode", "strict"]);
    assert_eq!(forced.status, 1, "{}", forced.json());

    fixture.write(
        "input.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );
    let directive = fixture.run(&[
        "check",
        "input.luau",
        "--json",
        "--set",
        "checking.mode=\"nocheck\"",
    ]);
    assert_eq!(directive.status, 0, "{}", directive.json());
    fixture.write(
        "instar.toml",
        "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n",
    );
    let directive = fixture.run(&["check", "input.luau", "--json"]);
    assert_eq!(directive.status, 1, "{}", directive.json());

    fixture.write("input.luau", "--!nonstrict\nlocal function value(input)\n    return input.missing\nend\nreturn value(1)\n");
    assert_eq!(fixture.run(&["check", "input.luau", "--json"]).status, 0);
    fixture.write("input.luau", "--!nocheck\nlocal =\n");
    let malformed = fixture.run(&["check", "input.luau", "--json"]);
    let malformed_report = malformed.json();
    assert_eq!(malformed.status, 1, "{malformed_report}");
    assert!(
        malformed_report["diagnostics"]
            .as_array()
            .expect("syntax diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Syntax")
    );
    fixture.write("input.luau", "return require('./dependency')\n");
    fixture.write(
        "dependency.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );
    let dependency = fixture.run(&["check", "input.luau", "--json", "--mode", "nocheck"]);
    let dependency_report = dependency.json();
    assert_eq!(dependency.status, 1, "{dependency_report}");
    assert_eq!(dependency_report["counts"]["dependencies"], 1);
    assert!(
        dependency_report["diagnostics"]
            .as_array()
            .expect("dependency mode findings")
            .iter()
            .any(
                |finding| finding["path"] == json!(fixture.path("dependency.luau"))
                    && finding["code"] == "1000"
            )
    );
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
    let valid = fixture.run(&["check", "probe.luau", "--json"]);
    assert_eq!(valid.status, 0, "{}", valid.json());
    for source in [
        "--!strict\nlocal value = 1 + 'wrong'\nreturn value\n",
        "--!strict\nlocal value = math.abs('wrong')\nreturn value\n",
    ] {
        fixture.write("probe.luau", source);
        let invalid = fixture.run(&["check", "probe.luau", "--json"]);
        let report = invalid.json();
        assert_eq!(invalid.status, 1, "{report}");
        assert_eq!(report["completion"], "Complete");
        let findings = report["diagnostics"]
            .as_array()
            .expect("invalid operations");
        assert_ne!(findings.as_slice(), [] as [Value; 0]);
        assert!(findings.iter().all(|finding| finding["category"] == "Type"
            && finding["path"] == json!(fixture.path("probe.luau"))));
        assert!(findings.iter().any(|finding| {
            finding["message"]
                .as_str()
                .is_some_and(|message| message.contains("number") && message.contains("string"))
        }));
    }
}

#[test]
fn partial() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("broken/init.luau", "--!strict\nlocal =\nreturn 1\n");
    fixture.write("probe.luau", "--!strict\nlocal dependency = require('./broken')\nlocal value: string = 12\nreturn dependency, value\n");
    let output = fixture.run(&["check", "probe.luau", "--json"]);
    let report = output.json();
    assert_ne!(output.status, 0, "{report}");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Syntax"
                && diagnostic["path"] == json!(fixture.path("broken/init.luau")))
    );
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Type"
                && diagnostic["path"] == json!(fixture.path("probe.luau")))
    );
    let prerequisite = report["diagnostics"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "analysis.dependency")
        .expect("dependency prerequisite");
    assert!(
        prerequisite["related"]
            .as_array()
            .expect("related locations")
            .iter()
            .any(|related| related["path"] == json!(fixture.path("broken/init.luau")))
    );
    snapshot(
        &fixture,
        "check",
        "partial",
        &json!({"diagnostics": report["diagnostics"], "sources": report["sources"], "completion": report["completion"], "status": report["status"]}),
    );
}

#[test]
fn profiles() {
    let fixture = Fixture::empty();
    fixture.write("instar.toml", "[checking]\nmode='strict'\n[environment]\nroblox=false\n[language.flags]\nLuauCyclicRequireTypeInference=true\n");
    fixture.write("input.luau", "return require('./nested/dependency')\n");
    fixture.write("nested/dependency.luau", "return {value = 1}\n");
    let compatible = fixture.run(&["check", "input.luau", "--json"]);
    let report = compatible.json();
    assert_eq!(compatible.status, 0, "{report}");
    assert!(report["settings"].as_array().expect("settings").iter().all(
        |effective| effective["settings"]["language"]["flags"]["LuauCyclicRequireTypeInference"]
            == true
    ));

    fixture.write(
        "nested/instar.toml",
        "[language.flags]\nLuauCyclicRequireTypeInference=false\n",
    );
    let incompatible = fixture.run(&["check", "input.luau", "--json"]);
    let failure = incompatible.json();
    assert_eq!(incompatible.status, 4, "{failure}");
    assert_ne!(failure["completion"], "Complete");
    assert_eq!(failure["counts"]["dependencies"], 1);
    assert_eq!(
        failure["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .filter(|diagnostic| diagnostic["code"] == "analysis.cross_profile")
            .count(),
        2
    );

    for setting in [
        "language.flags.AbsentFlag=true",
        "language.flags.LuauCyclicRequireTypeInference=1",
        "language.flags.DebugLuauLogSolver=true",
        "language.flags.DebugLuauTypeFunctionRuntimeHeapLimit=0",
    ] {
        let rejected = fixture.run(&["check", "input.luau", "--json", "--set", setting]);
        assert_eq!(rejected.status, 2, "{setting}: {}", rejected.json());
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
    let output = fixture.run(&["check", "first", "second", "--json"]);
    let report = output.json();
    assert_eq!(output.status, 0, "{report}");
    assert_eq!(report["counts"]["selected"], 2);
    let settings = report["settings"].as_array().expect("settings");
    assert!(
        settings
            .iter()
            .any(|effective| effective["settings"]["environment"]["globals"] == json!(["first"]))
    );
    assert!(
        settings
            .iter()
            .any(|effective| effective["settings"]["environment"]["globals"] == json!(["second"]))
    );

    fixture.write("second/input.lua", "return first\n");
    let crossed = fixture.run(&["check", "first", "second", "--json"]);
    let diagnostics = crossed.json();
    assert_eq!(crossed.status, 1, "{diagnostics}");
    assert!(
        diagnostics["diagnostics"]
            .as_array()
            .expect("findings")
            .iter()
            .all(|diagnostic| diagnostic["path"] == json!(fixture.path("second/input.lua")))
    );

    let explicit = fixture.run(&["check", "--project-root", "first", "--json"]);
    assert_eq!(explicit.status, 0, "{}", explicit.json());
    assert_eq!(explicit.json()["counts"]["selected"], 1);
    let outside = fixture.run(&[
        "check",
        "second/input.lua",
        "--project-root",
        "first",
        "--json",
    ]);
    assert_eq!(outside.status, 2, "{}", outside.json());

    fixture.write(
        "alternate.toml",
        "[checking]\nmode='nocheck'\n[environment]\nroblox=false\n",
    );
    let configured = fixture.run(&[
        "check",
        "second/input.lua",
        "--configuration",
        "alternate.toml",
        "--json",
    ]);
    assert_eq!(configured.status, 0, "{}", configured.json());
    assert_eq!(
        configured.json()["settings"][0]["settings"]["checking"],
        "nocheck"
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
    let output = fixture.run(&["check", "probe.luau", "--json"]);
    let report = output.json();
    assert_ne!(output.status, 0, "{report}");
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Syntax")
    );
    assert!(
        diagnostics
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Type" && diagnostic["code"] == "1000")
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
    let output = fixture.run(&["check", "probe.luau", "--json"]);
    let report = output.json();
    assert_eq!(output.status, 1, "{report}");
    let diagnostic = report["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| {
            diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Duplicate"))
        })
        .expect("duplicate type diagnostic");
    assert_eq!(diagnostic["related"].as_array().expect("related").len(), 1);
    assert_eq!(
        diagnostic["related"][0]["path"],
        json!(fixture.path("probe.luau"))
    );
    assert_eq!(
        diagnostic["related"][0]["message"],
        "Previous type definition"
    );
    assert_eq!(diagnostic["related"][0]["source"], diagnostic["source"]);
    assert!(
        diagnostic["related"][0]["range"][0]
            .as_u64()
            .expect("related start")
            < diagnostic["range"][0].as_u64().expect("primary start")
    );
    snapshot(
        &fixture,
        "check",
        "related",
        &json!({"diagnostics": report["diagnostics"], "status": report["status"], "completion": report["completion"]}),
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
    assert!(quiet_text.contains("1000"));
    assert!(quiet_text.contains("local value: string = 12"));
    assert!(!quiet_text.contains("Checking complete"));

    let colored = fixture.run(&["check", "probe.luau", "--color", "always"]);
    assert_eq!(colored.status, 1);
    assert!(format!("{}{}", colored.stdout, colored.stderr).contains("\u{1b}["));
    let automatic = fixture.run(&["check", "probe.luau", "--color", "auto"]);
    assert_eq!(automatic.status, 1);
    assert!(!format!("{}{}", automatic.stdout, automatic.stderr).contains('\u{1b}'));

    let structured = fixture.run(&["check", "probe.luau", "--report", "json", "--verbose"]);
    let report = structured.json();
    assert_eq!(structured.status, 1, "{report}");
    assert_eq!(report["operation"], "check");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["status"], 1);
    assert!(structured.stderr.lines().any(|line| {
        serde_json::from_str::<Value>(line).is_ok_and(|record| record["level"] == "DEBUG")
    }));
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
            "initial": {"sources": first.sources, "findings": first.findings},
            "unchanged": {"sources": second.sources, "findings": second.findings},
            "changed": {"sources": changed.sources, "findings": changed.findings},
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
    let supported = fixture.run(&["check", "input.luau", "--json"]);
    let report = supported.json();
    assert_eq!(supported.status, 0, "{report}");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["findings"], 0);
    assert_eq!(
        report["settings"][0]["settings"]["environment"]["roblox"],
        true
    );
    fixture.write("sourcemap.json", "{\"name\":\"Place\",\"className\":\"DataModel\",\"children\":[{\"name\":\"Library\",\"className\":\"Folder\",\"children\":[{\"name\":\"Caller\",\"className\":\"ModuleScript\",\"filePaths\":[\"mapped.luau\"]},{\"name\":\"Target\",\"className\":\"ModuleScript\",\"filePaths\":[\"target.luau\"]}]}]}\n");
    fixture.write("mapped.luau", "local target = require(script.Parent.Target)\nlocal value: target.Value = target\nlocal result: number = value.value\nreturn result\n");
    fixture.write(
        "target.luau",
        "export type Value = {value: number}\nreturn {value = 1}\n",
    );
    let mapped = fixture.run(&["check", "mapped.luau", "--json"]);
    let mapping = mapped.json();
    assert_eq!(mapped.status, 0, "{mapping}");
    assert_eq!(mapping["completion"], "Complete");
    assert_eq!(mapping["counts"]["dependencies"], 1);
    assert_eq!(
        mapping["dependencies"]
            .as_array()
            .expect("typed mapped closure")
            .len(),
        1
    );
    assert!(mapping["imports"][0]["failure"].is_null());

    fixture.write("input.luau", "local part: Part = Instance.new('Part')\npart.Name = 'forbidden'\nlocal world = Instance.new('DataModel')\nreturn part, world\n");
    let invalid = fixture.run(&["check", "input.luau", "--json"]);
    let failure = invalid.json();
    assert_eq!(invalid.status, 1, "{failure}");
    let findings = failure["diagnostics"]
        .as_array()
        .expect("capability findings");
    assert!(findings.iter().any(|diagnostic| {
        diagnostic["message"]
            .as_str()
            .is_some_and(|message| message.contains("Name") && message.contains("read"))
    }));
    assert!(findings.iter().any(
        |diagnostic| diagnostic["message"] == "Instance.new does not support class 'DataModel'"
    ));

    fixture.write("plain/instar.toml", "[environment]\nroblox=false\n");
    fixture.write(
        "plain/input.luau",
        "local part: Part = Instance.new('Part')\nreturn part\n",
    );
    let selected = fixture.run(&["check", "plain/input.luau", "--json"]);
    let plain = selected.json();
    assert_eq!(selected.status, 1, "{plain}");
    assert_eq!(
        plain["settings"][0]["settings"]["environment"]["roblox"],
        false
    );
    assert!(
        plain["diagnostics"]
            .as_array()
            .expect("unavailable globals")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("Instance")))
    );
    snapshot(
        &fixture,
        "check",
        "roblox",
        &json!({
            "supported": {"counts": report["counts"], "completion": report["completion"]},
            "capabilities": failure["diagnostics"],
            "plain": plain["diagnostics"],
            "mapping": {"dependencies": mapping["dependencies"], "counts": mapping["counts"], "completion": mapping["completion"]},
        }),
    );
}

#[test]
fn sandbox() {
    let fixture = Fixture::new("luau", "sandbox");
    fixture.write("input.luau", "local value: string = 12\nreturn value\n");
    let valid = fixture.run(&[
        "check",
        "input.luau",
        "--json",
        "--set",
        "environment.roblox=false",
    ]);
    let report = valid.json();
    assert_eq!(valid.status, 1, "{report}");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["settings"][0]["settings"]["checking"], "strict");
    assert_eq!(report["diagnostics"][0]["category"], "Type");

    fixture.write(
        "instar.luau",
        "os.execute('forbidden')\nreturn {checking = {mode = 'nocheck'}}\n",
    );
    let blocked = fixture.run(&["check", "input.luau", "--json"]);
    let failure = blocked.json();
    assert_eq!(blocked.status, 2, "{failure}");
    assert_ne!(failure["completion"], "Complete");
    assert!(
        failure["diagnostics"]
            .as_array()
            .expect("configuration diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Configuration")
    );
    assert!(
        failure["error"]
            .as_str()
            .expect("sandbox failure")
            .contains("instar.luau")
    );

    fixture.write("instar.toml", "[environment]\nroblox=false\n");
    let competing = fixture.run(&["check", "input.luau", "--json"]);
    let competing_report = competing.json();
    assert_eq!(competing.status, 2, "{competing_report}");
    assert_eq!(
        competing_report["diagnostics"]
            .as_array()
            .expect("competing diagnostics")
            .iter()
            .filter(|diagnostic| diagnostic["code"] == "configuration.competing")
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
    let discovered = fixture.run(&["check", "--json", "--explain-selection"]);
    let report = discovered.json();
    assert_eq!(discovered.status, 0, "{report}");
    assert_eq!(report["counts"]["selected"], 1);
    assert_eq!(report["counts"]["excluded"], 3);
    let decisions = report["selection"].as_array().expect("selection");
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
            .find(|decision| decision["input"] == json!(fixture.path(path)))
            .expect("selection decision");
        assert_eq!(decision["selected"], false);
        assert_eq!(decision["reason"], reason);
    }
    let ignore = decisions
        .iter()
        .find(|decision| decision["input"] == json!(fixture.path("source/ignored.luau")))
        .expect("ignore decision");
    assert_eq!(ignore["origin"], json!(fixture.path(".gitignore")));

    let explicit = fixture.run(&["check", "source/ignored.luau", "--json"]);
    assert_eq!(explicit.status, 1, "{}", explicit.json());
    assert_eq!(explicit.json()["counts"]["selected"], 1);
    let excluded = fixture.run(&["check", "source/excluded.luau", "--json"]);
    let empty = excluded.json();
    assert_eq!(excluded.status, 0, "{empty}");
    assert_eq!(empty["counts"]["selected"], 0);
    assert_eq!(empty["counts"]["excluded"], 1);
    assert_eq!(empty["selection"][0]["reason"], "configured exclusion");
    let override_selection = fixture.run(&[
        "check",
        "source/excluded.luau",
        "--json",
        "--override-exclusions",
    ]);
    assert_eq!(
        override_selection.status,
        1,
        "{}",
        override_selection.json()
    );
    assert_eq!(override_selection.json()["counts"]["selected"], 1);
    let command = fixture.run(&[
        "check",
        "--json",
        "--include",
        "outside.luau",
        "--exclude",
        "source/**",
    ]);
    assert_eq!(command.status, 1, "{}", command.json());
    assert_eq!(command.json()["counts"]["selected"], 1);
    assert_eq!(
        command.json()["diagnostics"][0]["path"],
        json!(fixture.path("outside.luau"))
    );
    snapshot(
        &fixture,
        "check",
        "selection",
        &json!({"selection": decisions, "counts": report["counts"], "empty": {"selection": empty["selection"], "counts": empty["counts"]}}),
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
            "--json",
        ],
        text,
    );
    let report = output.json();
    assert_eq!(output.status, 1, "{report}");
    assert_eq!(report["counts"]["selected"], 2);
    assert_eq!(report["counts"]["dependencies"], 2);
    let diagnostic = report["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["path"] == json!(fixture.path("nested/input.luau")))
        .expect("stdin identity");
    assert_eq!(diagnostic["category"], "Type");
    assert_eq!(diagnostic["code"], "1000");
    assert!(
        diagnostic["revision"]
            .as_u64()
            .is_some_and(|revision| revision > 0)
    );
    let start =
        usize::try_from(diagnostic["range"][0].as_u64().expect("start")).expect("byte start");
    let end = usize::try_from(diagnostic["range"][1].as_u64().expect("end")).expect("byte end");
    assert_eq!(&text[start..end], "library.double(2)");
    assert_eq!(fixture.bytes("nested/input.luau"), disk);

    let plain = fixture.stdin(
        &["check", "-", "--json", "--mode", "strict"],
        "local value: string = 12\nreturn value\n",
    );
    assert_eq!(plain.status, 1, "{}", plain.json());
    assert_eq!(plain.json()["diagnostics"][0]["path"], "stdin.luau");
    snapshot(
        &fixture,
        "check",
        "stdin",
        &json!({"diagnostics": report["diagnostics"], "selection": report["selection"], "dependencies": report["dependencies"], "counts": report["counts"], "completion": report["completion"]}),
    );
}

#[test]
fn streaming() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(
        "probe.luau",
        "--!strict\nlocal value: string = 12\nreturn value\n",
    );
    let output = fixture.run(&["check", "probe.luau", "--json", "--streaming"]);
    assert_eq!(output.status, 1, "{}", output.stderr);
    let records = output
        .stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stream record"))
        .collect::<Vec<_>>();
    let findings = records
        .iter()
        .filter(|record| record["event"] == "diagnostic")
        .collect::<Vec<_>>();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["diagnostic"]["code"], "1000");
    assert_eq!(
        findings[0]["diagnostic"]["path"],
        json!(fixture.path("probe.luau"))
    );
    let terminal = records.last().expect("terminal report");
    assert_eq!(terminal["event"], "report");
    assert_eq!(terminal["report"]["completion"], "Complete");
    assert_eq!(terminal["report"]["status"], 1);
    assert_eq!(
        terminal["report"]["diagnostics"][0],
        findings[0]["diagnostic"]
    );
    snapshot(
        &fixture,
        "check",
        "streaming",
        &json!({"findings": findings, "terminal": {"completion": terminal["report"]["completion"], "status": terminal["report"]["status"], "diagnostics": terminal["report"]["diagnostics"]}}),
    );
}

#[test]
fn timeout() {
    let fixture = Fixture::new("luau", "timeout");
    fixture.write("input.luau", "return 1\n");
    let configuration = fixture.bytes("instar.luau");
    let output = fixture.run(&["check", "input.luau", "--json", "--timeout", "1"]);
    let failure = output.json();
    assert_eq!(output.status, 4, "{failure}");
    assert_eq!(failure["status"], 4);
    assert_eq!(failure["completion"], "TimedOut");
    assert!(
        failure["error"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
    );
    assert_eq!(fixture.bytes("instar.luau"), configuration);
}

#[test]
fn typefunctions() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("probe.luau", "--!strict\ntype function identity(value)\n    return value\nend\nlocal value: identity<number> = 12\nlocal number: number = value\nreturn number\n");
    let valid = fixture.run(&["check", "probe.luau", "--json"]);
    assert_eq!(valid.status, 0, "{}", valid.json());
    fixture.write("probe.luau", "--!strict\ntype function identity(value)\n    return value\nend\nlocal value: identity<number> = 'wrong'\nreturn value\n");
    let invalid = fixture.run(&["check", "probe.luau", "--json"]);
    let report = invalid.json();
    assert_eq!(invalid.status, 1, "{report}");
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("type function findings")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "1000"
                && diagnostic["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("number")))
    );

    fixture.write("probe.luau", "--!strict\ntype function forbidden(value)\n    gcinfo()\n    return value\nend\nlocal value: forbidden<number> = 12\nreturn value\n");
    let sandboxed = fixture.run(&["check", "probe.luau", "--json"]);
    let failure = sandboxed.json();
    assert_eq!(sandboxed.status, 1, "{failure}");
    assert!(
        failure["diagnostics"]
            .as_array()
            .expect("sandbox diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["message"]
                .as_str()
                .is_some_and(|message| message.contains("not supported in type functions")))
    );
    fixture.write("probe.luau", "--!strict\ntype function exhausted()\n    local value = string.rep('x', 128 * 1024 * 1024)\n    return types.singleton(value)\nend\nlocal value: exhausted<> = 'text'\nreturn value\n");
    let exhausted = fixture.run(&["check", "probe.luau", "--json"]);
    let incomplete = exhausted.json();
    assert_eq!(exhausted.status, 4, "{incomplete}");
    assert_ne!(incomplete["completion"], "Complete");
    assert!(
        incomplete["diagnostics"]
            .as_array()
            .expect("resource diagnostics")
            .iter()
            .any(
                |diagnostic| diagnostic["message"].as_str().is_some_and(|message| message
                    .contains("memory")
                    && message.contains("type function"))
            )
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
    let output = fixture.run(&["check", "unicode.luau", "--json"]);
    let report = output.json();
    assert_eq!(output.status, 1, "{report}");
    let diagnostic = report["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "1000")
        .expect("type mismatch");
    let start =
        usize::try_from(diagnostic["range"][0].as_u64().expect("start")).expect("byte start");
    let end = usize::try_from(diagnostic["range"][1].as_u64().expect("end")).expect("byte end");
    assert_eq!(&text[start..end], "12");
    assert_eq!(start, text.find("12").expect("literal byte offset"));
    assert_eq!(diagnostic["path"], json!(fixture.path("unicode.luau")));
    assert_eq!(
        diagnostic["source"],
        diagnostic["identity"]["location"]["source"]
    );
    assert_eq!(diagnostic["revision"], diagnostic["source"]["revision"]);
    assert_eq!(fixture.bytes("unicode.luau"), bytes);
    snapshot(
        &fixture,
        "check",
        "unicode",
        &json!({"diagnostics": report["diagnostics"], "sources": report["sources"], "completion": report["completion"], "status": report["status"]}),
    );
}

#[test]
fn unresolved() {
    let fixture = Fixture::new("cli", "project");
    let text = "--!strict\nlocal dependency = require('./absent')\nlocal value: string = 12\nreturn dependency, value\n";
    fixture.write("probe.luau", text);
    let output = fixture.run(&["check", "probe.luau", "--json"]);
    let report = output.json();
    assert_eq!(output.status, 1, "{report}");
    let findings = report["diagnostics"].as_array().expect("findings");
    let unresolved = findings
        .iter()
        .find(|finding| finding["code"] == "resolution.unresolved")
        .expect("unresolved import");
    assert_eq!(unresolved["category"], "Resolution");
    let start = usize::try_from(unresolved["range"][0].as_u64().expect("start")).expect("start");
    let end = usize::try_from(unresolved["range"][1].as_u64().expect("end")).expect("end");
    assert_eq!(&text[start..end], "'./absent'");
    assert!(
        findings
            .iter()
            .any(|finding| finding["code"] == "1000" && finding["category"] == "Type")
    );
    assert_eq!(report["counts"]["dependencies"], 0);
    assert_eq!(report["imports"][0]["outcome"], "Unresolved");
    assert!(
        report["imports"][0]["failure"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty())
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
                "original": {"findings": original.findings, "sources": original.sources, "failed": original.failed},
                "updated": {"findings": updated.findings, "sources": updated.sources, "failed": updated.failed, "changed": second.iterations[0].changed},
            }),
        );
    }
}
