use std::{
    collections::BTreeSet,
    fs,
    num::NonZeroUsize,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use instar_format::{FormatOutcome, FormatScope, settings::IndentStyle};
use instar_project::{
    ApplyRequest, ChangeBatch, FormatMode, FormatOverrides, FormatRequest, Input, Invocation,
    ProjectChange, SourceChange, WatchCommandReport, WatchOperation, WatchRequest,
    WriteAuthorization, WriteOutcome,
};
use instar_shared::{Completion, EventPayload};
use serde_json::{Value, json};

use super::support::{Fixture, complete, options};

fn request(fixture: &Fixture) -> FormatRequest {
    FormatRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(fixture.path("input.luau"))],
            ..Invocation::default()
        },
        scope: FormatScope::Whole,
        overrides: FormatOverrides::default(),
        mode: FormatMode::Check,
        snapshot: None,
    }
}

fn validated(source: &Value, expected: &str) {
    assert_eq!(source["completion"], "Complete");
    assert_eq!(source["outcome"]["state"], "validated");
    assert_eq!(source["outcome"]["output"].as_str(), Some(expected));
    let preservation = &source["outcome"]["preservation"];
    assert_eq!(preservation["completion"], "Complete");
    for guarantee in [
        "structure",
        "literals",
        "comments",
        "directives",
        "multiple_results",
        "outside",
    ] {
        assert_eq!(preservation[guarantee], "Preserved", "{guarantee}");
    }
    if !source["proposal"].is_null() {
        assert_eq!(source["proposal"]["intent"], "Formatting");
        assert!(
            source["proposal"]["guards"]
                .as_array()
                .expect("proposal guards")
                .contains(&source["source"])
        );
        for edit in source["proposal"]["edits"]
            .as_array()
            .expect("proposal edits")
        {
            assert_eq!(edit["location"]["source"], source["source"]);
        }
    }
}

#[test]
fn retained_goldens_preserve_settings_and_bytes() {
    for scenario in [
        "calls/collapse",
        "calls/compact",
        "calls/surrounding",
        "calls/width",
        "identity/annotation",
        "identity/global",
        "literals/interpolation",
        "literals/long",
        "quotes/apostrophe",
        "quotes/double",
        "quotes/preference",
        "quotes/preserve",
        "quotes/single",
        "requires/alphabetical",
        "requires/comments",
        "requires/compact",
        "requires/custom",
        "requires/grouped",
        "requires/preserve",
        "spacing/blocks",
        "spacing/spaces",
        "tables/collapse",
        "tables/wrap",
        "trivia/blocks",
        "trivia/comments",
        "trivia/endings",
        "trivia/gaps",
        "trivia/preservation",
        "trivia/separators",
        "trivia/table",
        "types/collapse",
        "types/declarations",
        "types/wrap",
        "width/combining",
        "width/emoji",
        "width/exact",
        "width/indentation",
        "width/tabs",
        "width/wide",
        "width/wrap",
    ] {
        let fixture = Fixture::new("format", scenario);
        if fixture.path("configuration.toml").exists() {
            let configuration = fixture
                .read("configuration.toml")
                .replace("[requires]", "[formatting.requires]");
            fixture.write("instar.toml", format!("[formatting]\n{configuration}"));
        }
        let original = fixture.bytes("input.luau");
        let expected = fixture.bytes("expected.luau");
        let formatted = fixture.run(&["format", "--source", "input.luau"]);
        assert_eq!(formatted.status, 0, "{scenario}: {}", formatted.stderr);
        assert_eq!(formatted.stdout.as_bytes(), expected, "{scenario}");
        let repeated = fixture.run(&["format", "--source", "input.luau"]);
        assert_eq!(repeated.status, 0, "{scenario}: {}", repeated.stderr);
        assert_eq!(repeated.stdout.as_bytes(), expected, "{scenario}");
        let idempotent = fixture.stdin(
            &["format", "--source", "--stdin-filepath", "input.luau", "-"],
            &fixture.read("expected.luau"),
        );
        assert_eq!(idempotent.status, 0, "{scenario}: {}", idempotent.stderr);
        assert_eq!(idempotent.stdout.as_bytes(), expected, "{scenario}");
        let checked = fixture.run(&["format", "--json", "--check", "expected.luau"]);
        assert_eq!(checked.status, 0, "{scenario}: {}", checked.stderr);
        let report = checked.json();
        assert_eq!(report["counts"]["unchanged"], 1, "{scenario}");
        assert_eq!(report["counts"]["dependencies"], 0, "{scenario}");
        validated(&report["sources"][0], &fixture.read("expected.luau"));
        assert_eq!(fixture.bytes("input.luau"), original, "{scenario}");
        assert_eq!(fixture.bytes("expected.luau"), expected, "{scenario}");
    }
}

#[test]
fn whole_source_preserves_multiple_results_and_attributes() {
    for scenario in ["basic", "semantics", "attributes", "encoding"] {
        let fixture = Fixture::new("cli", &format!("format/{scenario}"));
        let original = fixture.bytes("input.luau");
        let output = fixture.run(&["format", "--json", "--check", "input.luau"]);
        assert_eq!(output.status, 1, "{scenario}: {}", output.stderr);
        let report = output.json();
        validated(&report["sources"][0], &fixture.read("expected.luau"));
        assert_eq!(report["counts"]["changed"], 1);
        assert_eq!(report["counts"]["written"], 0);
        assert_eq!(report["sources"][0]["write"], "NotRequested");
        assert_eq!(fixture.bytes("input.luau"), original);
    }
}

#[test]
fn check_and_preview_never_write() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let checked = fixture.run(&["format", "--json", "--check", "input.luau"]);
    assert_eq!(checked.status, 1);
    let report = checked.json();
    assert_eq!(report["mode"], "Check");
    assert_eq!(report["failed"], true);
    validated(&report["sources"][0], &fixture.read("expected.luau"));
    let previewed = fixture.run(&["format", "--json", "--preview", "input.luau"]);
    assert_eq!(previewed.status, 0);
    let preview = previewed.json();
    assert_eq!(preview["mode"], "Preview");
    assert_eq!(preview["completion"], "Complete");
    assert_eq!(preview["counts"]["written"], 0);
    let patch = preview["sources"][0]["preview"].as_str().expect("patch");
    assert!(patch.contains("-local value=1"));
    assert!(patch.contains("+\tlocal value = 1"));
    assert_eq!(preview["sources"][0]["write"], "NotRequested");
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn explicit_write_commits_validated_goldens() {
    let fixture = Fixture::new("cli", "format/basic");
    let written = fixture.run(&["format", "--json", "--write", "input.luau"]);
    assert_eq!(written.status, 0, "{}", written.stderr);
    let report = written.json();
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["written"], 1);
    assert_eq!(report["counts"]["unsuccessful"], 0);
    let source = &report["sources"][0];
    validated(source, &fixture.read("expected.luau"));
    assert_eq!(source["write"]["Written"]["source"], source["source"]);
    assert!(
        source["write"]["Written"]["revision"]
            .as_u64()
            .expect("written revision")
            > source["source"]["revision"]
                .as_u64()
                .expect("original revision")
    );
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    let unchanged = fixture.run(&["format", "--json", "--write", "input.luau"]);
    assert_eq!(unchanged.status, 0);
    assert_eq!(unchanged.json()["sources"][0]["write"], "Unchanged");
    assert_eq!(unchanged.json()["counts"]["written"], 0);
}

#[test]
fn unchanged_write_leaves_metadata_and_bytes_untouched() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("expected.luau");
    let modified = fs::metadata(fixture.path("expected.luau"))
        .expect("golden metadata")
        .modified()
        .expect("golden modification time");
    let output = fixture.run(&["format", "--json", "--write", "expected.luau"]);
    assert_eq!(output.status, 0);
    let report = output.json();
    assert_eq!(report["counts"]["unchanged"], 1);
    assert_eq!(report["counts"]["written"], 0);
    assert_eq!(report["sources"][0]["write"], "Unchanged");
    assert_eq!(fixture.bytes("expected.luau"), original);
    assert_eq!(
        fs::metadata(fixture.path("expected.luau"))
            .expect("golden metadata")
            .modified()
            .expect("golden modification time"),
        modified
    );
}

#[test]
fn batch_writes_retain_valid_and_unsuccessful_outcomes() {
    let fixture = Fixture::new("cli", "format/basic");
    let malformed = Fixture::new("cli", "format/recovery");
    fixture.write("broken.luau", malformed.bytes("input.luau"));
    let original = fixture.bytes("broken.luau");
    let output = fixture.run(&[
        "format",
        "--json",
        "--write",
        "input.luau",
        "expected.luau",
        "broken.luau",
    ]);
    assert_eq!(output.status, 5);
    let report = output.json();
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["selected"], 3);
    assert_eq!(report["counts"]["written"], 1);
    assert_eq!(report["counts"]["unchanged"], 1);
    assert_eq!(report["counts"]["unsuccessful"], 1);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    assert_eq!(fixture.bytes("broken.luau"), original);
    let failed = report["sources"]
        .as_array()
        .expect("sources")
        .iter()
        .find(|source| {
            source["path"]
                .as_str()
                .expect("path")
                .ends_with("broken.luau")
        })
        .expect("malformed source");
    assert_eq!(failed["outcome"]["state"], "unavailable");
    assert!(failed["proposal"].is_null());
    assert_eq!(failed["write"], "NotRequested");
}

#[test]
fn ranges_preserve_outside_bytes_and_original_coordinates() {
    let fixture = Fixture::new("cli", "format/ranges");
    let original = fixture.read("input.luau");
    let start = original.find("  local first").expect("first line");
    let end = original.find("  local second").expect("second line");
    let position = original.find("first").expect("first binding") + 2;
    let range = format!("{position}:{}", position + 1);
    let output = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--indentation",
        "spaces",
        "--indent-width",
        "2",
        "--range",
        &range,
        "input.luau",
    ]);
    assert_eq!(output.status, 1);
    let report = output.json();
    let source = &report["sources"][0];
    let expected = fixture.read("expected.luau");
    validated(source, &expected);
    assert_eq!(
        source["outcome"]["selections"],
        json!([{
            "requested": {"start": position, "end": position + 1},
            "expanded": [{"start": start, "end": end}], "outcome": "Formatted",
        }])
    );
    assert_eq!(&expected.as_bytes()[..start], &original.as_bytes()[..start]);
    assert!(expected.as_bytes().ends_with(&original.as_bytes()[end..]));
    assert_eq!(fixture.read("input.luau"), original);
}

#[test]
fn overlapping_ranges_merge_against_one_snapshot() {
    let fixture = Fixture::new("cli", "format/ranges");
    let original = fixture.read("input.luau");
    let first = original.find("first").expect("first binding");
    let second = original.find("second").expect("second binding");
    let start = original.find("  local first").expect("first line");
    let end = original.find("end\n").expect("block end");
    let ranges = [
        format!("{first}:{}", first + 1),
        format!("{}:{}", first + 1, first + 4),
        format!("{second}:{}", second + 1),
    ];
    let output = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--indentation",
        "spaces",
        "--indent-width",
        "2",
        "--range",
        &ranges[0],
        "--range",
        &ranges[1],
        "--range",
        &ranges[2],
        "input.luau",
    ]);
    assert_eq!(output.status, 1);
    let report = output.json();
    let source = &report["sources"][0];
    validated(source, &fixture.read("merged.luau"));
    assert_eq!(
        source["outcome"]["selections"]
            .as_array()
            .expect("selections")
            .len(),
        3
    );
    assert_eq!(
        source["outcome"]["preservation"]["selections"],
        json!([{"start": start, "end": end}])
    );
    assert_eq!(
        source["outcome"]["selections"][0]["expanded"],
        source["outcome"]["selections"][1]["expanded"]
    );
    assert_eq!(
        source["outcome"]["selections"][2]["requested"],
        json!({"start": second, "end": second + 1})
    );
    assert_eq!(fixture.read("input.luau"), original);
}

#[test]
fn on_type_localizes_the_callers_trigger() {
    let fixture = Fixture::new("cli", "format/ranges");
    let original = fixture.read("input.luau");
    let position = original.find("  local second").expect("line after cursor");
    let trigger = format!("{position}:\n");
    let output = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--indentation",
        "spaces",
        "--indent-width",
        "2",
        "--on-type",
        &trigger,
        "input.luau",
    ]);
    assert_eq!(output.status, 1);
    let report = output.json();
    validated(&report["sources"][0], &fixture.read("expected.luau"));
    let start = original.find("  local first").expect("trigger line");
    assert_eq!(
        report["sources"][0]["outcome"]["selections"][0]["requested"],
        json!({"start": start, "end": position})
    );
    assert_eq!(fixture.read("input.luau"), original);
}

#[test]
fn recovered_source_formats_only_well_formed_selected_regions() {
    let fixture = Fixture::new("cli", "format/recovery");
    let original = fixture.bytes("input.luau");
    let selected = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--range",
        "6:8",
        "input.luau",
    ]);
    assert_eq!(selected.status, 1);
    let report = selected.json();
    validated(&report["sources"][0], &fixture.read("expected.luau"));
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("syntax diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "syntax.parse")
    );
    let malformed = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--range",
        "20:25",
        "input.luau",
    ]);
    assert_eq!(malformed.status, 5);
    let unavailable = malformed.json();
    assert_eq!(unavailable["sources"][0]["outcome"]["state"], "unavailable");
    assert!(unavailable["sources"][0]["outcome"].get("output").is_none());
    assert!(unavailable["sources"][0]["proposal"].is_null());
    let whole = fixture.run(&["format", "--source", "input.luau"]);
    assert_eq!(whole.status, 5);
    assert_eq!(whole.stdout, "");
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn suppression_scopes_preserve_directives_and_protected_bytes() {
    let fixture = Fixture::new("cli", "format/suppressions");
    let original = fixture.bytes("input.luau");
    let output = fixture.run(&["format", "--json", "--check", "input.luau"]);
    assert_eq!(output.status, 1);
    let report = output.json();
    validated(&report["sources"][0], &fixture.read("expected.luau"));
    let whole = fixture.run(&["format", "--source", "whole.luau"]);
    assert_eq!(whole.status, 0);
    assert_eq!(whole.stdout.as_bytes(), fixture.bytes("whole.luau"));
    let selected = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--range",
        "40:48",
        "input.luau",
    ]);
    assert_eq!(selected.status, 0);
    let unchanged = selected.json();
    validated(&unchanged["sources"][0], &fixture.read("input.luau"));
    assert_eq!(
        unchanged["sources"][0]["outcome"]["selections"][0]["outcome"],
        "Unchanged"
    );
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn invalid_directives_are_source_attributed_failures() {
    let fixture = Fixture::new("cli", "format/directives");
    for (name, message) in [
        ("unknown.luau", "unknown formatting directive"),
        ("opening.luau", "unmatched formatting region start"),
        ("closing.luau", "unmatched formatting region end"),
        ("nested.luau", "nested formatting region"),
        (
            "orphan.luau",
            "formatting directive has no following statement",
        ),
    ] {
        let original = fixture.bytes(name);
        let output = fixture.run(&["format", "--json", "--write", name]);
        assert_eq!(output.status, 5, "{name}");
        let report = output.json();
        let source = &report["sources"][0];
        assert_eq!(source["outcome"]["state"], "unavailable");
        assert!(source["outcome"].get("output").is_none());
        assert!(source["proposal"].is_null());
        assert_eq!(source["write"], "NotRequested");
        let diagnostic = report["diagnostics"]
            .as_array()
            .expect("diagnostics")
            .iter()
            .find(|diagnostic| diagnostic["code"] == "format.directive")
            .expect("directive diagnostic");
        assert_eq!(diagnostic["category"], "Formatting");
        assert_eq!(diagnostic["message"], message);
        assert_eq!(diagnostic["source"], source["source"]);
        assert_eq!(diagnostic["revision"], source["source"]["revision"]);
        assert!(
            diagnostic["path"]
                .as_str()
                .expect("diagnostic path")
                .ends_with(name)
        );
        let text = fixture.read(name);
        let start = if name == "nested.luau" {
            text.find('\n').expect("opening directive") + 1
        } else {
            text.rfind("-- instar-format").expect("directive comment")
        };
        let end = start + text[start..].find('\n').expect("directive line end");
        assert_eq!(diagnostic["range"], json!([start, end]));
        let repeated = fixture.run(&["format", "--json", "--check", name]);
        assert_eq!(repeated.status, 5);
        let repeated = repeated.json();
        let identity = &repeated["diagnostics"]
            .as_array()
            .expect("repeated diagnostics")
            .iter()
            .find(|finding| finding["code"] == "format.directive" && finding["message"] == message)
            .expect("repeated directive diagnostic")["identity"];
        assert_eq!(&diagnostic["identity"], identity);
        assert_eq!(fixture.bytes(name), original);
    }
}

#[test]
fn nested_directive_diagnostic_identifies_the_opening_location() {
    let fixture = Fixture::new("cli", "format/directives");
    let output = fixture.run(&["format", "--json", "--check", "nested.luau"]);
    assert_eq!(output.status, 5);
    let report = output.json();
    let diagnostic = report["diagnostics"]
        .as_array()
        .expect("diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["message"] == "nested formatting region")
        .expect("nested directive diagnostic");
    assert!(
        diagnostic["related"]
            .as_array()
            .expect("related locations")
            .iter()
            .any(|related| {
                related["source"] == diagnostic["source"]
                    && related["range"][0] == 0
                    && related["range"][1] == 20
                    && !related["message"]
                        .as_str()
                        .expect("related explanation")
                        .is_empty()
            })
    );
}

#[test]
fn selection_combines_inputs_and_honors_project_controls() {
    let fixture = Fixture::new("cli", "format/contexts");
    let defaults = fixture.run(&["format", "--json", "--check", "--explain-selection"]);
    assert_eq!(defaults.status, 1);
    let report = defaults.json();
    assert_eq!(report["counts"]["selected"], 3);
    assert_eq!(report["counts"]["excluded"], 2);
    assert_eq!(report["counts"]["dependencies"], 0);
    let sources = report["sources"].as_array().expect("sources");
    let names = sources
        .iter()
        .map(|source| source["path"].as_str().expect("path").replace('\\', "/"))
        .collect::<BTreeSet<_>>();
    assert!(names.iter().any(|name| name.ends_with("/input.luau")));
    assert!(names.iter().any(|name| name.ends_with("/child/input.luau")));
    assert!(names.iter().any(|name| name.ends_with("/other.lua")));
    assert!(!names.iter().any(|name| name.ends_with("/ignored.luau")));
    assert!(!names.iter().any(|name| name.ends_with("/excluded.luau")));
    assert!(
        report["selection"]
            .as_array()
            .expect("selection")
            .iter()
            .any(|decision| {
                decision["reason"] == "configured exclusion" && decision["selected"] == false
            })
    );
    let combined = fixture.run(&[
        "format",
        "--json",
        "--check",
        "input.luau",
        "child",
        "*.lua",
    ]);
    assert_eq!(combined.status, 1);
    assert_eq!(combined.json()["counts"]["selected"], 3);
    let ignored = fixture.run(&["format", "--json", "--check", "ignored.luau"]);
    assert_eq!(ignored.status, 1);
    assert_eq!(ignored.json()["counts"]["selected"], 1);
    let excluded = fixture.run(&["format", "--json", "--check", "excluded.luau"]);
    assert_eq!(excluded.status, 0);
    assert_eq!(excluded.json()["counts"]["selected"], 0);
    assert_eq!(excluded.json()["counts"]["excluded"], 1);
    let overridden = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--override-exclusions",
        "excluded.luau",
    ]);
    assert_eq!(overridden.status, 1);
    assert_eq!(overridden.json()["counts"]["selected"], 1);
    let included = fixture.run(&["format", "--json", "--check", "--include", "*.lua", "."]);
    assert_eq!(included.status, 1);
    assert_eq!(included.json()["counts"]["selected"], 1);
    let excluded_command =
        fixture.run(&["format", "--json", "--check", "--exclude", "child/", "."]);
    assert_eq!(excluded_command.status, 1);
    assert_eq!(excluded_command.json()["counts"]["selected"], 3);
    assert!(
        !excluded_command.json()["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source["path"]
                .as_str()
                .expect("path")
                .replace('\\', "/")
                .ends_with("/child/input.luau"))
    );
}

#[test]
fn inherited_settings_keep_origins_and_command_overrides() {
    let fixture = Fixture::new("cli", "format/contexts");
    let original = fixture.bytes("instar.toml");
    let child = fixture.run(&["format", "--source", "child/input.luau"]);
    assert_eq!(child.status, 0);
    assert_eq!(child.stdout, "do\n    local text = \"child\"\nend\n");
    let parent = fixture.run(&["format", "--source", "input.luau"]);
    assert_eq!(parent.status, 0);
    assert_eq!(parent.stdout, "do\n  local text = 'parent'\nend\n");
    let contextual = fixture.stdin(
        &[
            "format",
            "--source",
            "--stdin-filepath",
            "child/input.luau",
            "-",
        ],
        &fixture.read("child/input.luau"),
    );
    assert_eq!(contextual.status, 0);
    assert_eq!(contextual.stdout, "do\n    local text = \"child\"\nend\n");
    let overridden = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--explain-settings",
        "--indent-width",
        "6",
        "--quotes",
        "force-single",
        "child/input.luau",
    ]);
    assert_eq!(overridden.status, 1);
    let report = overridden.json();
    validated(
        &report["sources"][0],
        "do\n      local text = 'child'\nend\n",
    );
    let settings = &report["settings"][0];
    assert_eq!(
        settings["source"],
        report["sources"][0]["source"]["identity"]
    );
    assert_eq!(settings["settings"]["formatting"]["indent_style"], "spaces");
    assert_eq!(settings["settings"]["formatting"]["indent_width"], 6);
    assert_eq!(settings["settings"]["checking"], "strict");
    assert!(
        settings["settings"]["environment"]["globals"]
            .as_array()
            .expect("globals")
            .contains(&json!("emit"))
    );
    let origins = settings["origins"].as_array().expect("setting origins");
    assert!(origins.iter().any(|origin| {
        origin["field"] == "formatting.indent_width" && origin["input"].is_null()
    }));
    assert!(origins.iter().any(|origin| {
        origin["field"] == "formatting.indent_style" && !origin["location"].is_null()
    }));
    let explicit = fixture.run(&[
        "format",
        "--source",
        "--configuration",
        "explicit.toml",
        "child/input.luau",
    ]);
    assert_eq!(explicit.status, 0);
    assert_eq!(explicit.stdout, "do\n\tlocal text = 'child'\nend\n");
    let rooted = fixture.run(&["format", "--json", "--check", "--project-root", "child"]);
    assert_eq!(rooted.status, 1);
    assert_eq!(rooted.json()["counts"]["selected"], 1);
    assert_eq!(fixture.bytes("instar.toml"), original);
}

#[test]
fn executable_configuration_is_evaluated_and_sandboxed() {
    let fixture = Fixture::new("cli", "format/executable");
    let output = fixture.run(&["format", "--source", "input.luau"]);
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout, "do\n  local text = 'configured'\nend\n");
    let sandbox = Fixture::new("cli", "format/sandbox");
    let original = sandbox.bytes("input.luau");
    let rejected = sandbox.run(&["format", "--json", "--check", "input.luau"]);
    assert_eq!(rejected.status, 2);
    let report = rejected.json();
    assert!(
        report["diagnostics"]
            .as_array()
            .expect("configuration diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["category"] == "Configuration")
    );
    assert!(!sandbox.path("escaped.luau").exists());
    assert_eq!(sandbox.bytes("input.luau"), original);
    let exhaustion = Fixture::new("cli", "format/exhaustion");
    let timed = exhaustion.run(&[
        "format",
        "--json",
        "--timeout",
        "1",
        "--check",
        "input.luau",
    ]);
    assert_eq!(timed.status, 4);
    assert_eq!(timed.json()["completion"], "TimedOut");
    assert!(
        timed
            .json()
            .get("sources")
            .is_none_or(|sources| sources.as_array().is_some_and(Vec::is_empty))
    );
    assert_eq!(exhaustion.bytes("input.luau"), original);
}

#[test]
fn source_output_is_isolated_and_stdin_never_authorizes_context_writes() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let expected = fixture.bytes("expected.luau");
    let output = fixture.stdin(
        &[
            "format",
            "--source",
            "--json",
            "--verbose",
            "--output",
            "report.json",
            "--stdin-filepath",
            "input.luau",
            "-",
        ],
        &fixture.read("input.luau"),
    );
    assert_eq!(output.status, 0);
    assert!(output.stderr.contains("Command finished"));
    assert_eq!(output.stdout.as_bytes(), expected);
    let report: Value =
        serde_json::from_str(&fixture.read("report.json")).expect("destination report");
    validated(&report["sources"][0], &fixture.read("expected.luau"));
    assert!(
        report["sources"][0]["path"]
            .as_str()
            .expect("context path")
            .ends_with("input.luau")
    );
    assert_eq!(report["sources"][0]["write"], "NotRequested");
    let write = fixture.stdin(
        &[
            "format",
            "--write",
            "--source",
            "--quiet",
            "--stdin-filepath",
            "input.luau",
            "-",
        ],
        &fixture.read("input.luau"),
    );
    assert_eq!(write.status, 0);
    assert_eq!(write.stdout.as_bytes(), expected);
    assert_eq!(fixture.bytes("input.luau"), original);
    let implicit = fixture.stdin(&["format", "-"], &fixture.read("input.luau"));
    assert_eq!(implicit.status, 0);
    assert_eq!(implicit.stdout.as_bytes(), expected);
    let quiet = fixture.run(&["format", "--source", "--quiet", "input.luau"]);
    assert_eq!(quiet.status, 0);
    assert_eq!(quiet.stdout.as_bytes(), expected);
    assert_eq!(quiet.stderr, "");
}

#[test]
fn combined_stdin_and_disk_sources_keep_distinct_identities() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let output = fixture.stdin(
        &[
            "format",
            "--json",
            "--check",
            "--stdin-filepath",
            "input.luau",
            "-",
            "input.luau",
        ],
        &fixture.read("expected.luau"),
    );
    assert_eq!(output.status, 1);
    let report = output.json();
    assert_eq!(report["counts"]["selected"], 2);
    assert_eq!(report["counts"]["changed"], 1);
    assert_eq!(report["counts"]["unchanged"], 1);
    let sources = report["sources"]
        .as_array()
        .expect("disk and stdin sources");
    assert_ne!(
        sources[0]["source"]["identity"],
        sources[1]["source"]["identity"]
    );
    for source in sources {
        validated(source, &fixture.read("expected.luau"));
    }
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn report_destinations_cannot_replace_sources_or_stdin_context() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let disk = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--output",
        "input.luau",
        "input.luau",
    ]);
    assert_eq!(disk.status, 2);
    assert_eq!(fixture.bytes("input.luau"), original);
    let stdin = fixture.stdin(
        &[
            "format",
            "--source",
            "--json",
            "--output",
            "input.luau",
            "--stdin-filepath",
            "input.luau",
            "-",
        ],
        &fixture.read("input.luau"),
    );
    assert_eq!(stdin.status, 2);
    assert_eq!(stdin.stdout, "");
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn invalid_invocations_and_empty_selections_are_explicit() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    for arguments in [
        vec![
            "format",
            "--json",
            "--check",
            "--range",
            "9:1",
            "input.luau",
        ],
        vec![
            "format",
            "--json",
            "--check",
            "--range",
            "0:9999",
            "input.luau",
        ],
        vec![
            "format",
            "--json",
            "--check",
            "--on-type",
            "1:ab",
            "input.luau",
        ],
        vec!["format", "--json", "--check", "--width", "0", "input.luau"],
        vec![
            "format",
            "--json",
            "--check",
            "--indent-width",
            "0",
            "input.luau",
        ],
        vec![
            "format",
            "--json",
            "--check",
            "--include",
            "!*.luau",
            "input.luau",
        ],
        vec!["format", "--source", "input.luau", "expected.luau"],
        vec!["format", "--source", "--json", "input.luau"],
        vec!["format", "--source", "--write", "input.luau"],
        vec!["format", "--source", "--watch", "input.luau"],
        vec!["format", "--json", "--check", "absent*.luau"],
        vec![
            "format",
            "--check",
            "--range",
            "0:1",
            "--on-type",
            "1:a",
            "input.luau",
        ],
        vec!["format", "--check", "--jobs", "0", "input.luau"],
        vec!["format", "--check", "--timeout", "0", "input.luau"],
    ] {
        let output = fixture.run(&arguments);
        assert_eq!(output.status, 2, "{arguments:?}: {}", output.stderr);
        assert_eq!(output.stdout, "", "{arguments:?}");
        assert_ne!(output.stderr, "", "{arguments:?}");
        assert_eq!(fixture.bytes("input.luau"), original);
    }
    let missing = fixture.run(&["format", "--json", "--check", "missing.luau"]);
    assert_ne!(missing.status, 0);
    let missing_report = missing.json();
    assert_eq!(missing_report["status"], missing.status);
    assert_ne!(
        missing_report["error"]
            .as_str()
            .expect("missing source failure"),
        ""
    );
    assert_eq!(
        missing_report["completed_sources"]
            .as_array()
            .expect("completed sources")
            .as_slice(),
        [] as [Value; 0]
    );
    let operational = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--output",
        "absent/report.json",
        "input.luau",
    ]);
    assert_eq!(operational.status, 3);
    assert_eq!(operational.json()["status"], 3);
    assert_ne!(operational.json()["completion"], "Complete");
    assert_eq!(operational.stdout, "");
    assert_eq!(fixture.bytes("input.luau"), original);
    let empty = Fixture::empty().run(&["format", "--json", "--check"]);
    assert_eq!(empty.status, 2);
    assert!(
        empty.json()["error"]
            .as_str()
            .expect("empty target explanation")
            .contains("empty discovery")
    );
    let excluded = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--exclude",
        "*.luau",
        "input.luau",
    ]);
    assert_eq!(excluded.status, 0);
    assert_eq!(excluded.json()["counts"]["selected"], 0);
    assert_eq!(excluded.json()["counts"]["excluded"], 1);
    assert_eq!(excluded.json()["completion"], "Complete");
}

#[test]
fn unsupported_encoding_and_utf8_range_boundaries_never_produce_output() {
    let fixture = Fixture::new("cli", "format/encoding");
    let original = fixture.bytes("input.luau");
    let text = fixture.read("input.luau");
    let position = text.find('é').expect("multibyte literal") + 1;
    let range = format!("{position}:{position}");
    let boundary = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--range",
        &range,
        "input.luau",
    ]);
    assert_eq!(boundary.status, 2);
    assert_eq!(boundary.stdout, "");
    assert!(
        boundary.json()["error"]
            .as_str()
            .expect("UTF-8 boundary error")
            .to_ascii_lowercase()
            .contains("utf-8")
    );
    assert_eq!(fixture.bytes("input.luau"), original);
    for (name, bytes, detail) in [
        (
            "unsupported.luau",
            vec![0xff, 0xfe, b'l', 0, b'o', 0],
            "encoding is unsupported",
        ),
        ("invalid.luau", vec![b'l', 0xff, b'\n'], "invalid utf-8"),
    ] {
        fixture.write(name, &bytes);
        let output = fixture.run(&["format", "--json", "--write", name]);
        assert_eq!(output.status, 2);
        assert_eq!(output.stdout, "");
        assert!(
            output.json()["error"]
                .as_str()
                .expect("encoding error")
                .to_ascii_lowercase()
                .contains(detail)
        );
        assert_eq!(fixture.bytes(name), bytes);
    }
}

#[test]
fn selected_language_context_reports_unavailable_syntax() {
    let fixture = Fixture::new("format", "types/declarations");
    let original = fixture.bytes("input.luau");
    let output = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--set",
        "language.declaration_syntax=false",
        "input.luau",
    ]);
    assert_eq!(output.status, 4);
    let report = output.json();
    assert_ne!(report["completion"], "Complete");
    assert_eq!(report["counts"]["unsuccessful"], 1);
    assert_eq!(report["sources"][0]["outcome"]["state"], "unavailable");
    assert!(report["sources"][0]["outcome"].get("output").is_none());
    let diagnostic = report["diagnostics"]
        .as_array()
        .expect("syntax diagnostics")
        .iter()
        .find(|diagnostic| diagnostic["code"] == "syntax.declaration")
        .expect("disabled declaration diagnostic");
    assert_eq!(diagnostic["source"], report["sources"][0]["source"]);
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn human_quiet_color_and_streaming_reports_retain_findings() {
    let fixture = Fixture::new("cli", "format/basic");
    let human = fixture.run(&["format", "--check", "--color", "never", "input.luau"]);
    assert_eq!(human.status, 1);
    let human_report = format!("{}{}", human.stdout, human.stderr);
    assert!(human_report.contains("Would reformat"));
    assert!(human_report.contains("1 would change"));
    assert!(human_report.contains("completion Complete"));
    assert!(!human_report.contains('\u{1b}'));
    let quiet = fixture.run(&["format", "--check", "--quiet", "input.luau"]);
    assert_eq!(quiet.status, 1);
    let quiet_report = format!("{}{}", quiet.stdout, quiet.stderr);
    assert!(quiet_report.contains("Would reformat"));
    assert!(!quiet_report.contains("Formatting complete"));
    let malformed = Fixture::new("cli", "format/recovery");
    for color in ["auto", "always", "never"] {
        let output = malformed.run(&["format", "--check", "--color", color, "input.luau"]);
        assert_eq!(output.status, 5);
        let rendered = format!("{}{}", output.stdout, output.stderr);
        assert!(rendered.contains("syntax.parse"));
        if color == "always" {
            assert!(rendered.contains('\u{1b}'));
        } else {
            assert!(!rendered.contains('\u{1b}'));
        }
    }
    let streamed = fixture.run(&[
        "format",
        "--json",
        "--streaming",
        "--check",
        "input.luau",
        "expected.luau",
    ]);
    assert_eq!(streamed.status, 1);
    let records = streamed
        .stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stream record"))
        .collect::<Vec<_>>();
    let terminal = records.last().expect("terminal report");
    assert_eq!(terminal["event"], "report");
    assert_eq!(terminal["report"]["completion"], "Complete");
    assert_eq!(terminal["report"]["counts"]["selected"], 2);
    for source in terminal["report"]["sources"].as_array().expect("sources") {
        assert!(records[..records.len() - 1].iter().any(|record| {
            record["event"]["payload"]["SourceOutcome"]["source"] == source["source"]
                && record["event"]["payload"]["SourceOutcome"]["completion"] == "Complete"
        }));
    }
}

#[test]
fn retained_snapshots_keep_proposals_on_the_analyzed_revision() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let first = complete(project.format(request(&fixture), options())).expect("initial formatting");
    assert_eq!(first.completion, Completion::Complete);
    let version = first.results[0].result.source;
    let original = first
        .snapshot
        .source(version.identity)
        .expect("captured source");
    assert_eq!(original.text.as_bytes(), fixture.bytes("input.luau"));
    let changed = project
        .change(ChangeBatch {
            changes: vec![ProjectChange::Source(SourceChange::Replace {
                expected: version,
                text: Arc::from(fixture.read("expected.luau")),
            })],
        })
        .expect("source revision change");
    assert_eq!(
        changed
            .source(version.identity)
            .expect("new source")
            .version
            .revision,
        version.revision + 1
    );
    let mut historical = request(&fixture);
    historical.snapshot = Some(first.snapshot.clone());
    let retained = complete(project.format(historical, options())).expect("historical formatting");
    assert_eq!(retained.results[0].result.source, version);
    assert_eq!(
        retained.results[0]
            .proposal
            .as_ref()
            .expect("historical proposal")
            .proposal()
            .guards,
        vec![version]
    );
    assert_eq!(
        first
            .snapshot
            .source(version.identity)
            .expect("retained source")
            .text,
        original.text
    );
    assert_eq!(fixture.bytes("input.luau"), original.text.as_bytes());
    assert!(!project.current(&first.inputs));
    let stale = complete(
        project.apply(
            ApplyRequest {
                edit: first.results[0]
                    .proposal
                    .as_ref()
                    .expect("original proposal")
                    .clone(),
                destinations: vec![WriteAuthorization {
                    source: version,
                    destination: fixture.path("input.luau"),
                }],
            },
            options(),
        ),
    )
    .expect_err("changed project revision must reject application");
    assert_ne!(stale.message, "");
    assert_eq!(stale.completed_edits, []);
    assert_eq!(fixture.bytes("input.luau"), original.text.as_bytes());
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn stale_sources_and_unauthorized_destinations_refuse_application() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let formatted =
        complete(project.format(request(&fixture), options())).expect("validated formatting");
    let source = formatted.results[0].result.source;
    let edit = formatted.results[0]
        .proposal
        .as_ref()
        .expect("validated proposal")
        .clone();
    let unauthorized = complete(project.apply(
        ApplyRequest {
            edit: edit.clone(),
            destinations: vec![WriteAuthorization {
                source,
                destination: fixture.path("expected.luau"),
            }],
        },
        options(),
    ))
    .expect_err("unrelated destination must fail");
    assert!(unauthorized.message.contains("not authorized"));
    let mut external = fixture.bytes("expected.luau");
    external.extend(fixture.bytes("input.luau"));
    fixture.write("input.luau", &external);
    let applied = complete(project.apply(
        ApplyRequest {
            edit,
            destinations: vec![WriteAuthorization {
                source,
                destination: fixture.path("input.luau"),
            }],
        },
        options(),
    ))
    .expect("explicit write failure report");
    assert_ne!(applied.completion, Completion::Complete);
    assert!(
        matches!(&applied.writes[..], [WriteOutcome::Failed { source: failed, message }]
        if *failed == source && message.contains("source changed"))
    );
    assert_eq!(applied.revisions, []);
    assert_eq!(fixture.bytes("input.luau"), external);
    assert_eq!(
        fixture.bytes("expected.luau"),
        formatted.results[0]
            .proposal
            .as_ref()
            .expect("proposal")
            .proposal()
            .edits[0]
            .replacement
            .as_bytes()
    );
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn conflicting_and_stale_proposals_fail_validation() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let report =
        complete(project.format(request(&fixture), options())).expect("formatting proposal");
    let validated = report.results[0].proposal.as_ref().expect("proposal");
    let mut conflicting = validated.proposal().clone();
    let sources = report.snapshot.sources().cloned().collect::<Vec<_>>();
    conflicting
        .validate(&sources)
        .expect("valid original proposal");
    conflicting.edits.push(conflicting.edits[0].clone());
    assert!(
        conflicting
            .validate(&sources)
            .expect_err("overlapping edits")
            .to_string()
            .contains("overlap")
    );
    let mut stale = validated.proposal().clone();
    stale.guards[0].revision += 1;
    assert!(
        stale
            .validate(&sources)
            .expect_err("stale guard")
            .to_string()
            .contains("stale")
    );
    let mut invalid = validated.proposal().clone();
    invalid.edits[0].location.span.start = invalid.edits[0].location.span.end + 1;
    assert!(invalid.validate(&sources).is_err());
    assert_eq!(
        fixture.bytes("input.luau"),
        report
            .snapshot
            .source(report.results[0].result.source.identity)
            .expect("original source")
            .text
            .as_bytes()
    );
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn cancellation_and_expired_budgets_never_expose_partial_source() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let project = fixture.project();
    let mut cancelled = options();
    let cancellation = cancelled.cancellation.clone();
    cancelled.observer = Some(Arc::new(move |event| {
        if matches!(&event.payload, EventPayload::Progress(progress) if progress.phase == "parsing")
        {
            cancellation.cancel();
        }
    }));
    match complete(project.format(request(&fixture), cancelled)) {
        Ok(report) => {
            assert_eq!(report.completion, Completion::Cancelled);
            assert!(report.results.iter().all(|source| {
                matches!(&source.result.outcome, FormatOutcome::Unavailable { .. })
                    && source.proposal.is_none()
            }));
        }
        Err(failure) => {
            assert_eq!(failure.completion, Completion::Cancelled);
            assert_eq!(failure.completed_sources, []);
            assert_eq!(failure.completed_edits, []);
        }
    }
    let mut expired = options();
    expired.deadline = Some(Instant::now());
    let failure = complete(project.format(request(&fixture), expired)).expect_err("expired budget");
    assert_eq!(failure.completion, Completion::TimedOut);
    assert_eq!(failure.completed_sources, []);
    assert_eq!(failure.completed_edits, []);
    assert_eq!(fixture.bytes("input.luau"), original);
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn interrupted_batch_retains_completed_source_outcomes() {
    let fixture = Fixture::new("cli", "format/basic");
    fixture.write("other.luau", fixture.bytes("input.luau"));
    let project = fixture.project();
    let mut requested = request(&fixture);
    requested
        .invocation
        .inputs
        .push(Input::Path(fixture.path("other.luau")));
    let mut interrupted = options();
    interrupted.jobs = NonZeroUsize::MIN;
    let cancellation = interrupted.cancellation.clone();
    interrupted.observer = Some(Arc::new(move |event| {
        if matches!(event.payload, EventPayload::SourceOutcome(_)) {
            cancellation.cancel();
        }
    }));
    let failure =
        complete(project.format(requested, interrupted)).expect_err("interrupted second source");
    assert_eq!(failure.completion, Completion::Cancelled);
    assert_eq!(failure.completed_sources.len(), 1);
    assert_eq!(
        failure.completed_sources[0].completion,
        Completion::Complete
    );
    assert_eq!(failure.completed_edits, []);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("other.luau"));
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn jobs_bound_formatting_and_progress_preserves_source_identity() {
    let fixture = Fixture::new("cli", "format/basic");
    fixture.write("other.luau", fixture.bytes("input.luau"));
    let project = fixture.project();
    let mut requested = request(&fixture);
    requested
        .invocation
        .inputs
        .push(Input::Path(fixture.path("other.luau")));
    let active = Arc::new(AtomicUsize::new(0));
    let maximum = Arc::new(AtomicUsize::new(0));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed_active = Arc::clone(&active);
    let observed_maximum = Arc::clone(&maximum);
    let observed_events = Arc::clone(&events);
    let mut execution = options();
    execution.jobs = NonZeroUsize::MIN;
    execution.observer = Some(Arc::new(move |event| {
        if let EventPayload::Progress(progress) = &event.payload {
            if progress.phase == "parsing" && progress.completed == 0 {
                let concurrent = observed_active.fetch_add(1, Ordering::SeqCst) + 1;
                observed_maximum.fetch_max(concurrent, Ordering::SeqCst);
            } else if progress.phase == "preservation" && progress.completed == 1 {
                observed_active.fetch_sub(1, Ordering::SeqCst);
            }
        }
        observed_events
            .lock()
            .expect("event collection")
            .push(event);
    }));
    let report = complete(project.format(requested, execution)).expect("bounded formatting");
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.results.len(), 2);
    assert_eq!(maximum.load(Ordering::SeqCst), 1);
    assert_eq!(active.load(Ordering::SeqCst), 0);
    let events = events.lock().expect("events");
    assert!(events.iter().any(|event| matches!(&event.payload,
        EventPayload::Progress(progress) if progress.source.is_none())));
    for source in &report.results {
        for phase in ["parsing", "formatting", "preservation"] {
            assert!(events.iter().any(|event| matches!(&event.payload,
                EventPayload::Progress(progress) if progress.phase == phase
                    && progress.source == Some(source.result.source))));
        }
    }
    drop(events);
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn unchanged_complete_inputs_reuse_formatting_work() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let parsing = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&parsing);
    let mut execution = options();
    execution.observer = Some(Arc::new(move |event| {
        if matches!(&event.payload, EventPayload::Progress(progress) if progress.phase == "parsing")
        {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let first =
        complete(project.format(request(&fixture), execution.clone())).expect("first formatting");
    assert_eq!(first.completion, Completion::Complete);
    assert_eq!(parsing.load(Ordering::SeqCst), 1);
    let second = complete(project.format(request(&fixture), execution)).expect("reused formatting");
    assert_eq!(second.completion, Completion::Complete);
    assert_eq!(
        second.results[0].result.source,
        first.results[0].result.source
    );
    match (
        &first.results[0].result.outcome,
        &second.results[0].result.outcome,
    ) {
        (FormatOutcome::Validated(first), FormatOutcome::Validated(second)) => {
            assert_eq!(first.output(), fixture.read("expected.luau"));
            assert_eq!(second.output(), first.output());
            assert_eq!(second.input(), first.input());
        }
        _ => panic!("both complete invocations must produce validated formatting"),
    }
    assert_eq!(
        parsing.load(Ordering::SeqCst),
        1,
        "unchanged complete inputs must reuse formatting"
    );
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn watch_reprocesses_source_and_effective_settings_changes() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let watched = WatchRequest {
        operation: WatchOperation::Format(request(&fixture)),
        poll_interval: Duration::from_millis(10),
    };
    let first = complete(project.watch(watched.clone(), options())).expect("initial watch report");
    assert_eq!(first.completion, Completion::Complete);
    assert_eq!(first.iterations.len(), 1);
    let WatchCommandReport::Format(first_format) = &first.iterations[0].report else {
        panic!("format watch must return a formatting report");
    };
    let original = first_format
        .snapshot
        .source(first_format.results[0].result.source.identity)
        .expect("watch source")
        .clone();
    fixture.write("input.luau", fixture.bytes("expected.luau"));
    let second =
        complete(project.watch(watched.clone(), options())).expect("source-triggered watch report");
    assert_eq!(second.completion, Completion::Complete);
    assert_eq!(second.iterations.len(), 1);
    assert_ne!(second.iterations[0].changed, []);
    let WatchCommandReport::Format(second_format) = &second.iterations[0].report else {
        panic!("source change must retain formatting report");
    };
    assert_eq!(second_format.counts.unchanged, 1);
    assert_eq!(
        second_format.results[0].result.source.identity,
        original.version.identity
    );
    assert_eq!(
        second_format.results[0].result.source.revision,
        original.version.revision + 1
    );
    fixture.write(
        "instar.toml",
        "[formatting]\nindent_style = \"spaces\"\nindent_width = 2\n",
    );
    let third =
        complete(project.watch(watched, options())).expect("settings-triggered watch report");
    assert_eq!(third.completion, Completion::Complete);
    assert_eq!(third.iterations.len(), 1);
    assert_ne!(third.iterations[0].changed, []);
    let WatchCommandReport::Format(third_format) = &third.iterations[0].report else {
        panic!("settings change must retain formatting report");
    };
    let FormatOutcome::Validated(formatted) = &third_format.results[0].result.outcome else {
        panic!("new settings must produce validated formatting");
    };
    assert_eq!(formatted.output(), "do\n  local value = 1\nend\n");
    assert_eq!(
        first_format
            .snapshot
            .source(original.version.identity)
            .expect("historical source")
            .text,
        original.text
    );
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn command_style_controls_change_only_layout() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let output = fixture.run(&[
        "format",
        "--source",
        "--indentation",
        "spaces",
        "--indent-width",
        "2",
        "--line-ending",
        "crlf",
        "--final-newline",
        "false",
        "input.luau",
    ]);
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.as_bytes(), b"do\r\n  local value = 1\r\nend");
    let newline = fixture.run(&[
        "format",
        "--source",
        "--line-ending",
        "lf",
        "--final-newline",
        "true",
        "input.luau",
    ]);
    assert_eq!(newline.status, 0);
    assert_eq!(newline.stdout.as_bytes(), fixture.bytes("expected.luau"));
    let width = Fixture::new("format", "tables/wrap");
    let wrapped = width.run(&["format", "--source", "--width", "36", "input.luau"]);
    assert_eq!(wrapped.status, 0);
    assert_eq!(wrapped.stdout.as_bytes(), width.bytes("expected.luau"));
    let imports = Fixture::new("format", "requires/alphabetical");
    let ordered = imports.run(&[
        "format",
        "--source",
        "--import-order",
        "alphabetical",
        "input.luau",
    ]);
    assert_eq!(ordered.status, 0);
    assert_eq!(ordered.stdout.as_bytes(), imports.bytes("expected.luau"));
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn combined_stdin_writes_only_the_authorized_disk_source() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.read("input.luau");
    let expected = fixture.bytes("expected.luau");
    let output = fixture.stdin(
        &[
            "format",
            "--write",
            "--json",
            "--output",
            "report.json",
            "--stdin-filepath",
            "input.luau",
            "-",
            "input.luau",
        ],
        &original,
    );
    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.as_bytes(), expected);
    let report: Value =
        serde_json::from_str(&fixture.read("report.json")).expect("combined report");
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["selected"], 2);
    assert_eq!(report["counts"]["written"], 1);
    let sources = report["sources"]
        .as_array()
        .expect("stdin and disk outcomes");
    assert_eq!(
        sources
            .iter()
            .filter(|source| source["write"] == "NotRequested")
            .count(),
        1
    );
    assert_eq!(
        sources
            .iter()
            .filter(|source| source["write"].get("Written").is_some())
            .count(),
        1
    );
    assert_ne!(
        sources[0]["source"]["identity"],
        sources[1]["source"]["identity"]
    );
    for source in sources {
        validated(source, &fixture.read("expected.luau"));
    }
    assert_eq!(fixture.bytes("input.luau"), expected);
    assert_eq!(fixture.bytes("expected.luau"), expected);
}

#[test]
fn effective_input_changes_invalidate_reused_formatting() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let first = complete(project.format(request(&fixture), options())).expect("initial format");
    let mut changed = request(&fixture);
    changed.overrides.formatting.indent_style = Some(IndentStyle::Spaces);
    changed.overrides.formatting.indent_width = NonZeroUsize::new(2);
    let second = complete(project.format(changed, options())).expect("overridden format");
    assert_eq!(second.completion, Completion::Complete);
    assert_eq!(
        second.results[0].result.source,
        first.results[0].result.source
    );
    let (FormatOutcome::Validated(original), FormatOutcome::Validated(overridden)) = (
        &first.results[0].result.outcome,
        &second.results[0].result.outcome,
    ) else {
        panic!("both contexts must produce validated formatting");
    };
    assert_eq!(original.output(), fixture.read("expected.luau"));
    assert_eq!(overridden.output(), "do\n  local value = 1\nend\n");
    assert_eq!(
        overridden.input().settings.indent_style,
        IndentStyle::Spaces
    );
    assert_ne!(overridden.input(), original.input());
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn nonregular_destinations_report_failed_writes_without_revisions() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let report = complete(project.format(request(&fixture), options())).expect("validated format");
    let source = report.results[0].result.source;
    let edit = report.results[0]
        .proposal
        .as_ref()
        .expect("proposal")
        .clone();
    fs::remove_file(fixture.path("input.luau")).expect("replace fixture destination");
    fs::create_dir(fixture.path("input.luau")).expect("nonregular destination");
    let applied = complete(project.apply(
        ApplyRequest {
            edit,
            destinations: vec![WriteAuthorization {
                source,
                destination: fixture.path("input.luau"),
            }],
        },
        options(),
    ))
    .expect("write failure report");
    assert_ne!(applied.completion, Completion::Complete);
    assert!(
        matches!(&applied.writes[..], [WriteOutcome::Failed { source: failed, message }]
        if *failed == source && message.contains("nonregular"))
    );
    assert_eq!(applied.revisions, []);
    assert!(fixture.path("input.luau").is_dir());
    assert_eq!(
        fixture.read("expected.luau"),
        "do\n\tlocal value = 1\nend\n"
    );
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn shared_typed_project_preserves_source_and_is_idempotent() {
    let fixture = Fixture::new("cli", "project");
    let goldens = Fixture::new("cli", "format/project");
    let names = ["input.luau", "library.luau", "consumer.lua"];
    let originals = names.map(|name| fixture.bytes(name));
    let output = fixture.run(&[
        "format",
        "--json",
        "--check",
        "--indentation",
        "spaces",
        "--indent-width",
        "4",
        "input.luau",
        "library.luau",
        "consumer.lua",
    ]);
    assert_eq!(output.status, 1, "{}", output.stderr);
    let report = output.json();
    assert_eq!(report["completion"], "Complete");
    assert_eq!(report["counts"]["selected"], 3);
    assert_eq!(report["counts"]["unchanged"], 0);
    assert_eq!(report["counts"]["changed"], 3);
    assert_eq!(report["counts"]["written"], 0);
    assert_eq!(report["counts"]["dependencies"], 0);
    for (name, original) in names.into_iter().zip(originals) {
        let source = report["sources"]
            .as_array()
            .expect("typed project sources")
            .iter()
            .find(|source| {
                source["path"]
                    .as_str()
                    .expect("source path")
                    .ends_with(name)
            })
            .expect("selected typed source");
        validated(source, &goldens.read(name));
        assert_eq!(fixture.bytes(name), original);
        let idempotent = fixture.stdin(
            &[
                "format",
                "--source",
                "--indentation",
                "spaces",
                "--indent-width",
                "4",
                "--stdin-filepath",
                name,
                "-",
            ],
            &goldens.read(name),
        );
        assert_eq!(idempotent.status, 0);
        assert_eq!(idempotent.stdout.as_bytes(), goldens.bytes(name));
    }
    let stdin = fixture.stdin(
        &[
            "format",
            "--source",
            "--indentation",
            "spaces",
            "--indent-width",
            "4",
            "--stdin-filepath",
            "input.luau",
            "-",
        ],
        &fixture.read("input.luau"),
    );
    assert_eq!(stdin.status, 0);
    assert_eq!(stdin.stdout.as_bytes(), goldens.bytes("input.luau"));
}

#[test]
fn interrupted_write_batch_retains_the_completed_commit() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    fixture.write("other.luau", &original);
    let project = fixture.project();
    let mut requested = request(&fixture);
    requested.mode = FormatMode::Write;
    requested
        .invocation
        .inputs
        .push(Input::Path(fixture.path("other.luau")));
    let mut interrupted = options();
    interrupted.jobs = NonZeroUsize::MIN;
    let cancellation = interrupted.cancellation.clone();
    interrupted.observer = Some(Arc::new(move |event| {
        if matches!(event.payload, EventPayload::EditOutcome(_)) {
            cancellation.cancel();
        }
    }));
    let failure =
        complete(project.format(requested, interrupted)).expect_err("second write interrupted");
    assert_eq!(failure.completion, Completion::Cancelled);
    assert_eq!(failure.completed_sources.len(), 1);
    assert_eq!(
        failure.completed_sources[0].completion,
        Completion::Complete
    );
    assert_eq!(failure.completed_edits.len(), 1);
    assert_eq!(failure.completed_edits[0].completion, Completion::Complete);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    assert_eq!(fixture.bytes("other.luau"), original);
    complete(project.finish()).expect("project cleanup");
}

#[test]
fn write_progress_reports_application_for_the_guarded_revision() {
    let fixture = Fixture::new("cli", "format/basic");
    let project = fixture.project();
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let mut execution = options();
    execution.observer = Some(Arc::new(move |event| {
        observed.lock().expect("write events").push(event);
    }));
    let mut requested = request(&fixture);
    requested.mode = FormatMode::Write;
    let report = complete(project.format(requested, execution)).expect("protected write");
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.written, 1);
    let source = report.results[0].result.source;
    assert!(matches!(
        report.results[0].write,
        WriteOutcome::Written { .. }
    ));
    let events = events.lock().expect("write events");
    assert!(events.iter().any(|event| {
        let EventPayload::Progress(progress) = &event.payload else {
            return false;
        };
        let phase = progress.phase.to_ascii_lowercase();
        progress.source == Some(source) && (phase.contains("appl") || phase.contains("writ"))
    }));
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    drop(events);
    complete(project.finish()).expect("project cleanup");
}
