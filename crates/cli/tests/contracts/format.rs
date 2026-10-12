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

use instar_format::{
    FormatOutcome, FormatScope, FormatSelection, SelectionOutcome, ValidatedFormat,
    settings::{IndentStyle, QuoteStyle},
};

use instar_project::{
    ApplyRequest, ChangeBatch, CheckingMode, FormatMode, FormatOverrides, FormatReport,
    FormatRequest, FormattedSource, Input, Invocation, ProjectChange, SourceChange,
    WatchCommandReport, WatchOperation, WatchRequest, WriteAuthorization, WriteOutcome,
};

use instar_shared::{Completion, DiagnosticCategory, EditIntent, EventPayload, Span};

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

fn format_report(fixture: &Fixture, requested: FormatRequest) -> FormatReport {
    let project = fixture.project();
    let report = complete(project.format(requested, options())).expect("formatting report");
    complete(project.finish()).expect("project cleanup");

    report
}

fn validated<'source>(
    source: &'source FormattedSource,
    expected: &str,
) -> &'source ValidatedFormat {
    assert_eq!(source.result.completion, Completion::Complete);

    let FormatOutcome::Validated(formatted) = &source.result.outcome else {
        panic!("validated formatting");
    };

    assert_eq!(formatted.output(), expected);
    assert_eq!(formatted.preservation().completion, Completion::Complete);
    assert!(formatted.preservation().preserved());

    if let Some(proposal) = &source.proposal {
        let proposal = proposal.proposal();
        assert_eq!(proposal.intent, EditIntent::Formatting);
        assert!(proposal.guards.contains(&source.result.source));

        for edit in &proposal.edits {
            assert_eq!(edit.location.source, source.result.source);
        }
    }

    formatted
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
        let checked = fixture.run(&["format", "--check", "expected.luau"]);
        assert_eq!(checked.status, 0, "{scenario}: {}", checked.stderr);
        let mut requested = request(&fixture);
        requested.invocation.inputs = vec![Input::Path(fixture.path("expected.luau"))];
        let report = format_report(&fixture, requested);
        assert_eq!(report.counts.unchanged, 1, "{scenario}");
        assert_eq!(report.counts.dependencies, 0, "{scenario}");
        validated(&report.results[0], &fixture.read("expected.luau"));
        assert_eq!(fixture.bytes("input.luau"), original, "{scenario}");
        assert_eq!(fixture.bytes("expected.luau"), expected, "{scenario}");
    }
}

#[test]
fn whole_source_preserves_multiple_results_and_attributes() {
    for scenario in ["basic", "semantics", "attributes", "encoding"] {
        let fixture = Fixture::new("cli", &format!("format/{scenario}"));
        let original = fixture.bytes("input.luau");
        let output = fixture.run(&["format", "--check", "input.luau"]);
        assert_eq!(output.status, 1, "{scenario}: {}", output.stderr);
        let report = format_report(&fixture, request(&fixture));
        validated(&report.results[0], &fixture.read("expected.luau"));
        assert_eq!(report.counts.changed, 1);
        assert_eq!(report.counts.written, 0);

        assert!(matches!(
            report.results[0].write,
            WriteOutcome::NotRequested
        ));

        assert_eq!(fixture.bytes("input.luau"), original);
    }
}

#[test]
fn check_and_preview_never_write() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");
    let checked = fixture.run(&["format", "--check", "input.luau"]);
    assert_eq!(checked.status, 1);
    let checked_report = format_report(&fixture, request(&fixture));
    assert!(checked_report.failed);
    validated(&checked_report.results[0], &fixture.read("expected.luau"));
    let previewed = fixture.run(&["format", "--preview", "input.luau"]);
    assert_eq!(previewed.status, 0);
    assert!(previewed.stderr.contains("-local value=1"));
    assert!(previewed.stderr.contains("+\tlocal value = 1"));
    let mut requested = request(&fixture);
    requested.mode = FormatMode::Preview;
    let preview = format_report(&fixture, requested);
    assert_eq!(preview.completion, Completion::Complete);
    assert_eq!(preview.counts.written, 0);
    let patch = preview.results[0].preview.as_deref().expect("patch");
    assert!(patch.contains("-local value=1"));
    assert!(patch.contains("+\tlocal value = 1"));

    assert!(matches!(
        preview.results[0].write,
        WriteOutcome::NotRequested
    ));

    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn default_formatting_commits_validated_goldens() {
    let fixture = Fixture::new("cli", "format/basic");
    let written = fixture.run(&["format", "input.luau"]);
    assert_eq!(written.status, 0, "{}", written.stderr);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    let unchanged = fixture.run(&["format", "input.luau"]);
    assert_eq!(unchanged.status, 0);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));

    let typed = Fixture::new("cli", "format/basic");
    let mut requested = request(&typed);
    requested.mode = FormatMode::Write;
    let written = format_report(&typed, requested.clone());
    assert_eq!(written.completion, Completion::Complete);
    assert_eq!(written.counts.written, 1);
    assert_eq!(written.counts.unsuccessful, 0);
    let source = &written.results[0];
    validated(source, &typed.read("expected.luau"));

    let WriteOutcome::Written {
        source: original,
        revision,
        ..
    } = source.write
    else {
        panic!("written source");
    };

    assert_eq!(original, source.result.source);
    assert!(revision > original.revision);
    assert_eq!(typed.bytes("input.luau"), typed.bytes("expected.luau"));
    let unchanged = format_report(&typed, requested);

    assert!(matches!(
        unchanged.results[0].write,
        WriteOutcome::Unchanged
    ));

    assert_eq!(unchanged.counts.written, 0);
}

#[test]
fn unchanged_write_leaves_metadata_and_bytes_untouched() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("expected.luau");

    let modified = fs::metadata(fixture.path("expected.luau"))
        .expect("golden metadata")
        .modified()
        .expect("golden modification time");

    let output = fixture.run(&["format", "expected.luau"]);
    assert_eq!(output.status, 0);
    let mut requested = request(&fixture);
    requested.invocation.inputs = vec![Input::Path(fixture.path("expected.luau"))];
    requested.mode = FormatMode::Write;
    let report = format_report(&fixture, requested);
    assert_eq!(report.counts.unchanged, 1);
    assert_eq!(report.counts.written, 0);
    assert!(matches!(report.results[0].write, WriteOutcome::Unchanged));
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

    let output = fixture.run(&["format", "input.luau", "expected.luau", "broken.luau"]);
    assert_eq!(output.status, 5);
    assert_eq!(fixture.bytes("input.luau"), fixture.bytes("expected.luau"));
    assert_eq!(fixture.bytes("broken.luau"), original);

    let typed = Fixture::new("cli", "format/basic");
    typed.write("broken.luau", &original);
    let mut requested = request(&typed);
    requested.mode = FormatMode::Write;

    requested.invocation.inputs = ["input.luau", "expected.luau", "broken.luau"]
        .map(|name| Input::Path(typed.path(name)))
        .into();

    let report = format_report(&typed, requested);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.selected, 3);
    assert_eq!(report.counts.written, 1);
    assert_eq!(report.counts.unchanged, 1);
    assert_eq!(report.counts.unsuccessful, 1);
    assert_eq!(typed.bytes("input.luau"), typed.bytes("expected.luau"));
    assert_eq!(typed.bytes("broken.luau"), original);

    let failed = report
        .results
        .iter()
        .find(|source| {
            report
                .snapshot
                .origin(source.result.source.identity)
                .and_then(|origin| origin.path())
                == Some(typed.path("broken.luau").as_path())
        })
        .expect("malformed source");

    assert!(matches!(
        failed.result.outcome,
        FormatOutcome::Unavailable { .. }
    ));

    assert!(failed.proposal.is_none());
    assert!(matches!(failed.write, WriteOutcome::NotRequested));
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
    let mut requested = request(&fixture);

    requested.scope = FormatScope::Ranges(vec![Span {
        start: position,
        end: position + 1,
    }]);

    requested.overrides.formatting.indent_style = Some(IndentStyle::Spaces);
    requested.overrides.formatting.indent_width = NonZeroUsize::new(2);
    let report = format_report(&fixture, requested);
    let expected = fixture.read("expected.luau");
    let formatted = validated(&report.results[0], &expected);

    assert_eq!(
        formatted.selections(),
        &[FormatSelection {
            requested: Span {
                start: position,
                end: position + 1
            },
            expanded: vec![Span { start, end }],
            outcome: SelectionOutcome::Formatted,
        }]
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
    let mut requested = request(&fixture);

    requested.scope = FormatScope::Ranges(vec![
        Span {
            start: first,
            end: first + 1,
        },
        Span {
            start: first + 1,
            end: first + 4,
        },
        Span {
            start: second,
            end: second + 1,
        },
    ]);

    requested.overrides.formatting.indent_style = Some(IndentStyle::Spaces);
    requested.overrides.formatting.indent_width = NonZeroUsize::new(2);
    let report = format_report(&fixture, requested);
    let formatted = validated(&report.results[0], &fixture.read("merged.luau"));
    assert_eq!(formatted.selections().len(), 3);

    assert_eq!(
        formatted.preservation().selections,
        vec![Span { start, end }]
    );

    assert_eq!(
        formatted.selections()[0].expanded,
        formatted.selections()[1].expanded
    );

    assert_eq!(
        formatted.selections()[2].requested,
        Span {
            start: second,
            end: second + 1
        }
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
    let mut requested = request(&fixture);

    requested.scope = FormatScope::OnType {
        offset: position,
        trigger: '\n',
    };

    requested.overrides.formatting.indent_style = Some(IndentStyle::Spaces);
    requested.overrides.formatting.indent_width = NonZeroUsize::new(2);
    let report = format_report(&fixture, requested);
    let formatted = validated(&report.results[0], &fixture.read("expected.luau"));
    let start = original.find("  local first").expect("trigger line");

    assert_eq!(
        formatted.selections()[0].requested,
        Span {
            start,
            end: position
        }
    );

    assert_eq!(fixture.read("input.luau"), original);
}

#[test]
fn recovered_source_formats_only_well_formed_selected_regions() {
    let fixture = Fixture::new("cli", "format/recovery");
    let original = fixture.bytes("input.luau");

    let selected = fixture.run(&["format", "--check", "--range", "6:8", "input.luau"]);

    assert_eq!(selected.status, 1);
    let mut requested = request(&fixture);
    requested.scope = FormatScope::Ranges(vec![Span { start: 6, end: 8 }]);
    let report = format_report(&fixture, requested);
    validated(&report.results[0], &fixture.read("expected.luau"));

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "syntax.parse")
    );

    let malformed = fixture.run(&["format", "--check", "--range", "20:25", "input.luau"]);

    assert_eq!(malformed.status, 5);
    let mut requested = request(&fixture);
    requested.scope = FormatScope::Ranges(vec![Span { start: 20, end: 25 }]);
    let unavailable = format_report(&fixture, requested);

    assert!(matches!(
        unavailable.results[0].result.outcome,
        FormatOutcome::Unavailable { .. }
    ));

    assert!(unavailable.results[0].proposal.is_none());
    let whole = fixture.run(&["format", "--source", "input.luau"]);
    assert_eq!(whole.status, 5);
    assert_eq!(whole.stdout, "");
    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn suppression_scopes_preserve_directives_and_protected_bytes() {
    let fixture = Fixture::new("cli", "format/suppressions");
    let original = fixture.bytes("input.luau");
    let output = fixture.run(&["format", "--check", "input.luau"]);
    assert_eq!(output.status, 1);
    let report = format_report(&fixture, request(&fixture));
    validated(&report.results[0], &fixture.read("expected.luau"));
    let whole = fixture.run(&["format", "--source", "whole.luau"]);
    assert_eq!(whole.status, 0);
    assert_eq!(whole.stdout.as_bytes(), fixture.bytes("whole.luau"));

    let selected = fixture.run(&["format", "--check", "--range", "40:48", "input.luau"]);

    assert_eq!(selected.status, 0);
    let mut requested = request(&fixture);
    requested.scope = FormatScope::Ranges(vec![Span { start: 40, end: 48 }]);
    let unchanged = format_report(&fixture, requested);
    let formatted = validated(&unchanged.results[0], &fixture.read("input.luau"));

    assert_eq!(
        formatted.selections()[0].outcome,
        SelectionOutcome::Unchanged
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
        let output = fixture.run(&["format", name]);
        assert_eq!(output.status, 5, "{name}");
        let mut requested = request(&fixture);
        requested.invocation.inputs = vec![Input::Path(fixture.path(name))];
        requested.mode = FormatMode::Write;
        let report = format_report(&fixture, requested.clone());
        let source = &report.results[0];

        assert!(matches!(
            source.result.outcome,
            FormatOutcome::Unavailable { .. }
        ));

        assert!(source.proposal.is_none());
        assert!(matches!(source.write, WriteOutcome::NotRequested));

        let diagnostic = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "format.directive")
            .expect("directive diagnostic");

        assert_eq!(diagnostic.category, DiagnosticCategory::Formatting);
        assert_eq!(diagnostic.message, message);
        assert_eq!(diagnostic.primary.source, source.result.source);

        assert_eq!(
            diagnostic.primary.source.revision,
            source.result.source.revision
        );

        assert_eq!(
            report
                .snapshot
                .origin(diagnostic.primary.source.identity)
                .and_then(|origin| origin.path()),
            Some(fixture.path(name).as_path())
        );

        let text = fixture.read(name);

        let start = if name == "nested.luau" {
            text.find('\n').expect("opening directive") + 1
        } else {
            text.rfind("-- instar-format").expect("directive comment")
        };

        let end = start + text[start..].find('\n').expect("directive line end");
        assert_eq!(diagnostic.primary.span, Span { start, end });
        let repeated = fixture.run(&["format", "--check", name]);
        assert_eq!(repeated.status, 5);
        requested.mode = FormatMode::Check;
        let repeated = format_report(&fixture, requested);

        let identity = &repeated
            .diagnostics
            .iter()
            .find(|finding| finding.code == "format.directive" && finding.message == message)
            .expect("repeated directive diagnostic")
            .identity;

        assert_eq!(&diagnostic.identity, identity);
        assert_eq!(fixture.bytes(name), original);
    }
}

#[test]
fn nested_directive_diagnostic_identifies_the_opening_location() {
    let fixture = Fixture::new("cli", "format/directives");
    let mut requested = request(&fixture);
    requested.invocation.inputs = vec![Input::Path(fixture.path("nested.luau"))];
    let report = format_report(&fixture, requested);
    assert!(report.failed);
    assert_eq!(report.completion, Completion::Complete);

    assert!(matches!(
        report.results[0].result.outcome,
        FormatOutcome::Unavailable { .. }
    ));

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.message == "nested formatting region")
        .expect("nested directive diagnostic");

    assert!(diagnostic.related.iter().any(|related| {
        related.location.source == diagnostic.primary.source
            && related.location.span == Span { start: 0, end: 20 }
            && !related.message.is_empty()
    }));
}

#[test]
fn selection_combines_inputs_and_honors_project_controls() {
    let fixture = Fixture::new("cli", "format/contexts");
    let defaults = fixture.run(&["format", "--check", "--explain-selection"]);
    assert_eq!(defaults.status, 1);
    assert!(defaults.stderr.contains("Selection:"));
    assert!(defaults.stderr.contains("configured exclusion"));
    let mut requested = request(&fixture);
    requested.invocation.inputs.clear();
    requested.invocation.explanations.selection = true;
    let report = format_report(&fixture, requested.clone());
    assert_eq!(report.counts.selected, 3);
    assert_eq!(report.counts.excluded, 2);
    assert_eq!(report.counts.dependencies, 0);

    let names = report
        .results
        .iter()
        .map(|source| {
            report
                .snapshot
                .origin(source.result.source.identity)
                .and_then(|origin| origin.path())
                .expect("source path")
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect::<BTreeSet<_>>();

    assert!(names.iter().any(|name| name.ends_with("/input.luau")));
    assert!(names.iter().any(|name| name.ends_with("/child/input.luau")));
    assert!(names.iter().any(|name| name.ends_with("/other.lua")));
    assert!(!names.iter().any(|name| name.ends_with("/ignored.luau")));
    assert!(!names.iter().any(|name| name.ends_with("/excluded.luau")));

    assert!(
        report
            .selection
            .iter()
            .any(|decision| { decision.reason == "configured exclusion" && !decision.selected })
    );

    let combined = fixture.run(&["format", "--check", "input.luau", "child", "*.lua"]);
    assert_eq!(combined.status, 1);

    requested.invocation.inputs = vec![
        Input::Path(fixture.path("input.luau")),
        Input::Path(fixture.path("child")),
        Input::Pattern {
            base: fixture.root().to_path_buf(),
            pattern: "*.lua".into(),
        },
    ];

    assert_eq!(
        format_report(&fixture, requested.clone()).counts.selected,
        3
    );

    let ignored = fixture.run(&["format", "--check", "ignored.luau"]);
    assert_eq!(ignored.status, 1);
    requested.invocation.inputs = vec![Input::Path(fixture.path("ignored.luau"))];

    assert_eq!(
        format_report(&fixture, requested.clone()).counts.selected,
        1
    );

    let excluded = fixture.run(&["format", "--check", "excluded.luau"]);
    assert_eq!(excluded.status, 0);
    requested.invocation.inputs = vec![Input::Path(fixture.path("excluded.luau"))];
    let excluded_report = format_report(&fixture, requested.clone());
    assert_eq!(excluded_report.counts.selected, 0);
    assert_eq!(excluded_report.counts.excluded, 1);

    let overridden = fixture.run(&[
        "format",
        "--check",
        "--override-exclusions",
        "excluded.luau",
    ]);

    assert_eq!(overridden.status, 1);
    requested.overrides.selection.override_exclusions = true;

    assert_eq!(
        format_report(&fixture, requested.clone()).counts.selected,
        1
    );

    let included = fixture.run(&["format", "--check", "--include", "*.lua", "."]);
    assert_eq!(included.status, 1);
    requested.invocation.inputs = vec![Input::Path(fixture.root().to_path_buf())];
    requested.overrides.selection.override_exclusions = false;
    requested.overrides.selection.include = Some(vec!["*.lua".into()]);

    assert_eq!(
        format_report(&fixture, requested.clone()).counts.selected,
        1
    );

    let excluded_command = fixture.run(&["format", "--check", "--exclude", "child/", "."]);
    assert_eq!(excluded_command.status, 1);
    requested.overrides.selection.include = None;
    requested.overrides.selection.exclude = Some(vec!["child/".into()]);
    let excluded_report = format_report(&fixture, requested);
    assert_eq!(excluded_report.counts.selected, 3);

    assert!(!excluded_report.results.iter().any(|source| {
        excluded_report
            .snapshot
            .origin(source.result.source.identity)
            .and_then(|origin| origin.path())
            == Some(fixture.path("child/input.luau").as_path())
    }));
}

#[test]
fn inherited_settings_keep_command_overrides() {
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
        "--check",
        "--indent-width",
        "6",
        "--quotes",
        "force-single",
        "child/input.luau",
    ]);

    assert_eq!(overridden.status, 1);
    let mut requested = request(&fixture);
    requested.invocation.inputs = vec![Input::Path(fixture.path("child/input.luau"))];
    requested.overrides.formatting.indent_width = NonZeroUsize::new(6);
    requested.overrides.formatting.quote_style = Some(QuoteStyle::Single);
    let report = format_report(&fixture, requested);
    validated(&report.results[0], "do\n      local text = 'child'\nend\n");
    let settings = &report.settings[0];
    assert_eq!(settings.source, report.results[0].result.source.identity);

    assert_eq!(
        settings.settings.formatting.indent_style,
        IndentStyle::Spaces
    );

    assert_eq!(settings.settings.formatting.indent_width.get(), 6);
    assert!(matches!(settings.settings.checking, CheckingMode::Strict));

    assert!(
        settings
            .settings
            .environment
            .globals
            .iter()
            .any(|name| name == "emit")
    );

    let explicit = fixture.run(&[
        "format",
        "--source",
        "--configuration",
        "explicit.toml",
        "child/input.luau",
    ]);

    assert_eq!(explicit.status, 0);
    assert_eq!(explicit.stdout, "do\n\tlocal text = 'child'\nend\n");
    let rooted = fixture.run(&["format", "--check", "--project-root", "child"]);
    assert_eq!(rooted.status, 1);
    let mut requested = request(&fixture);
    requested.invocation.inputs.clear();
    requested.invocation.project_root = Some(fixture.path("child"));
    assert_eq!(format_report(&fixture, requested).counts.selected, 1);
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
    let rejected = sandbox.run(&["format", "--check", "input.luau"]);
    assert_eq!(rejected.status, 2);
    assert!(rejected.stderr.contains("configuration"));
    let project = sandbox.project();

    let failure = complete(project.format(request(&sandbox), options()))
        .expect_err("sandboxed configuration");

    assert!(
        failure
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Configuration)
    );

    complete(project.finish()).expect("project cleanup");

    assert!(!sandbox.path("escaped.luau").exists());
    assert_eq!(sandbox.bytes("input.luau"), original);
    let exhaustion = Fixture::new("cli", "format/exhaustion");

    let timed = exhaustion.run(&["format", "--timeout", "1", "--check", "input.luau"]);

    assert_eq!(timed.status, 4);

    assert!(
        timed.stderr.contains("TimedOut")
            || timed.stderr.contains(&Completion::TimedOut.to_string()),
        "{}",
        timed.stderr
    );

    assert_eq!(timed.stdout, "");
    let project = exhaustion.project();
    let mut execution = options();
    execution.deadline = Some(Instant::now() + Duration::from_secs(1));

    match complete(project.format(request(&exhaustion), execution)) {
        Ok(report) => {
            assert_eq!(report.completion, Completion::TimedOut);
            assert!(report.results.is_empty());
        }

        Err(failure) => {
            assert_eq!(failure.completion, Completion::TimedOut);
            assert_eq!(failure.completed_sources, []);
        }
    }

    complete(project.finish()).expect("project cleanup");

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
            "--verbose",
            "--output",
            "report.txt",
            "--stdin-filepath",
            "input.luau",
            "-",
        ],
        &fixture.read("input.luau"),
    );

    assert_eq!(output.status, 0);
    assert!(output.stderr.contains("Command finished"));
    assert_eq!(output.stdout.as_bytes(), expected);

    let destination = fixture.read("report.txt");
    assert!(destination.contains("Formatting complete"));
    assert!(destination.contains("completion Complete"));
    let mut requested = request(&fixture);
    requested.mode = FormatMode::Source;

    requested.invocation.inputs = vec![Input::Stdin {
        text: Arc::from(fixture.read("input.luau")),
        context: Some(fixture.path("input.luau")),
    }];

    let report = format_report(&fixture, requested);
    let source = &report.results[0];
    validated(source, &fixture.read("expected.luau"));

    assert_eq!(
        report
            .snapshot
            .origin(source.result.source.identity)
            .and_then(|origin| origin.path()),
        Some(fixture.path("input.luau").as_path())
    );

    assert!(matches!(source.write, WriteOutcome::NotRequested));

    let write = fixture.stdin(
        &["format", "--quiet", "--stdin-filepath", "input.luau", "-"],
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
            "--check",
            "--stdin-filepath",
            "input.luau",
            "-",
            "input.luau",
        ],
        &fixture.read("expected.luau"),
    );

    assert_eq!(output.status, 1);
    let mut requested = request(&fixture);

    requested.invocation.inputs.push(Input::Stdin {
        text: Arc::from(fixture.read("expected.luau")),
        context: Some(fixture.path("input.luau")),
    });

    let report = format_report(&fixture, requested);
    assert_eq!(report.counts.selected, 2);
    assert_eq!(report.counts.changed, 1);
    assert_eq!(report.counts.unchanged, 1);

    assert_ne!(
        report.results[0].result.source.identity,
        report.results[1].result.source.identity
    );

    for source in &report.results {
        validated(source, &fixture.read("expected.luau"));
    }

    assert_eq!(fixture.bytes("input.luau"), original);
}

#[test]
fn report_destinations_cannot_replace_sources_or_stdin_context() {
    let fixture = Fixture::new("cli", "format/basic");
    let original = fixture.bytes("input.luau");

    let disk = fixture.run(&["format", "--check", "--output", "input.luau", "input.luau"]);

    assert_eq!(disk.status, 2);
    assert_eq!(fixture.bytes("input.luau"), original);

    let stdin = fixture.stdin(
        &[
            "format",
            "--check",
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
        vec!["format", "--check", "--range", "9:1", "input.luau"],
        vec!["format", "--check", "--range", "0:9999", "input.luau"],
        vec!["format", "--check", "--on-type", "1:ab", "input.luau"],
        vec!["format", "--check", "--width", "0", "input.luau"],
        vec!["format", "--check", "--indent-width", "0", "input.luau"],
        vec!["format", "--check", "--include", "!*.luau", "input.luau"],
        vec!["format", "--source", "input.luau", "expected.luau"],
        vec!["format", "--source", "--check", "input.luau"],
        vec!["format", "--source", "--watch", "input.luau"],
        vec!["format", "--check", "absent*.luau"],
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

    let missing = fixture.run(&["format", "--check", "missing.luau"]);
    assert_ne!(missing.status, 0);
    assert_ne!(missing.stderr, "");
    assert_eq!(missing.stdout, "");
    let project = fixture.project();
    let mut requested = request(&fixture);
    requested.invocation.inputs = vec![Input::Path(fixture.path("missing.luau"))];
    let failure = complete(project.format(requested, options())).expect_err("missing source");
    assert_ne!(failure.message, "");
    assert_eq!(failure.completed_sources, []);
    complete(project.finish()).expect("project cleanup");

    let operational = fixture.run(&[
        "format",
        "--check",
        "--output",
        "absent/report.txt",
        "input.luau",
    ]);

    assert_eq!(operational.status, 3);
    assert_ne!(operational.stderr, "");
    assert!(!fixture.path("absent/report.txt").exists());
    assert_eq!(operational.stdout, "");
    assert_eq!(fixture.bytes("input.luau"), original);
    let empty = Fixture::empty().run(&["format", "--check"]);
    assert_eq!(empty.status, 2);
    assert!(empty.stderr.contains("empty discovery"));

    let excluded = fixture.run(&["format", "--check", "--exclude", "*.luau", "input.luau"]);

    assert_eq!(excluded.status, 0);
    let mut requested = request(&fixture);
    requested.overrides.selection.exclude = Some(vec!["*.luau".into()]);
    let report = format_report(&fixture, requested);
    assert_eq!(report.counts.selected, 0);
    assert_eq!(report.counts.excluded, 1);
    assert_eq!(report.completion, Completion::Complete);
}

#[test]
fn unsupported_encoding_and_utf8_range_boundaries_never_produce_output() {
    let fixture = Fixture::new("cli", "format/encoding");
    let original = fixture.bytes("input.luau");
    let text = fixture.read("input.luau");
    let position = text.find('é').expect("multibyte literal") + 1;
    let range = format!("{position}:{position}");

    let boundary = fixture.run(&["format", "--check", "--range", &range, "input.luau"]);

    assert_eq!(boundary.status, 2);
    assert_eq!(boundary.stdout, "");

    assert!(boundary.stderr.to_ascii_lowercase().contains("utf-8"));

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
        let output = fixture.run(&["format", name]);
        assert_eq!(output.status, 2);
        assert_eq!(output.stdout, "");

        assert!(output.stderr.to_ascii_lowercase().contains(detail));

        assert_eq!(fixture.bytes(name), bytes);
    }
}

#[test]
fn selected_language_context_reports_unavailable_syntax() {
    let fixture = Fixture::new("format", "types/declarations");
    let original = fixture.bytes("input.luau");

    let output = fixture.run(&[
        "format",
        "--check",
        "--set",
        "language.declaration_syntax=false",
        "input.luau",
    ]);

    assert_eq!(output.status, 4);
    let mut requested = request(&fixture);
    requested.overrides.language.declaration_syntax = Some(false);
    let report = format_report(&fixture, requested);
    assert_ne!(report.completion, Completion::Complete);
    assert_eq!(report.counts.unsuccessful, 1);

    assert!(matches!(
        report.results[0].result.outcome,
        FormatOutcome::Unavailable { .. }
    ));

    let diagnostic = report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.code == "syntax.declaration")
        .expect("disabled declaration diagnostic");

    assert_eq!(diagnostic.primary.source, report.results[0].result.source);
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
        "--streaming",
        "--check",
        "input.luau",
        "expected.luau",
    ]);

    assert_eq!(streamed.status, 1);

    assert_eq!(streamed.stdout, "");
    assert!(streamed.stderr.contains("Would reformat"));
    assert!(streamed.stderr.contains("2 files"));
    assert!(streamed.stderr.contains("completion Complete"));
    let events = Arc::new(Mutex::new(Vec::new()));
    let observed = Arc::clone(&events);
    let mut execution = options();

    execution.observer = Some(Arc::new(move |event| {
        observed.lock().expect("streaming events").push(event);
    }));

    let project = fixture.project();
    let mut requested = request(&fixture);

    requested
        .invocation
        .inputs
        .push(Input::Path(fixture.path("expected.luau")));

    let report = complete(project.format(requested, execution)).expect("streaming source outcomes");
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.selected, 2);
    let events = events.lock().expect("source events");

    for source in &report.results {
        assert!(events.iter().any(|event| matches!(&event.payload,
            EventPayload::SourceOutcome(outcome) if outcome.source == source.result.source
                && outcome.completion == Completion::Complete)));
    }

    drop(events);
    complete(project.finish()).expect("project cleanup");

    let streamed = malformed.run(&[
        "format",
        "--streaming",
        "--check",
        "--color",
        "never",
        "input.luau",
    ]);

    assert_eq!(streamed.status, 5);
    assert_eq!(streamed.stdout, "");
    let terminal = malformed.run(&["format", "--check", "--color", "never", "input.luau"]);
    assert_eq!(terminal.status, 5);

    assert_eq!(
        streamed.stderr.matches("syntax.parse").count(),
        terminal.stderr.matches("syntax.parse").count()
    );

    assert!(streamed.stderr.contains("syntax.parse"));
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
            "--output",
            "report.txt",
            "--stdin-filepath",
            "input.luau",
            "-",
            "input.luau",
        ],
        &original,
    );

    assert_eq!(output.status, 0);
    assert_eq!(output.stdout.as_bytes(), expected);

    let destination = fixture.read("report.txt");
    assert!(destination.contains("Formatting complete"));
    assert!(destination.contains("2 files"));
    assert!(destination.contains("1 written"));
    let typed = Fixture::new("cli", "format/basic");
    let mut requested = request(&typed);
    requested.mode = FormatMode::Write;

    requested.invocation.inputs.push(Input::Stdin {
        text: Arc::from(original),
        context: Some(typed.path("input.luau")),
    });

    let report = format_report(&typed, requested);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.selected, 2);
    assert_eq!(report.counts.written, 1);

    assert_eq!(
        report
            .results
            .iter()
            .filter(|source| matches!(source.write, WriteOutcome::NotRequested))
            .count(),
        1
    );

    assert_eq!(
        report
            .results
            .iter()
            .filter(|source| matches!(source.write, WriteOutcome::Written { .. }))
            .count(),
        1
    );

    assert_ne!(
        report.results[0].result.source.identity,
        report.results[1].result.source.identity
    );

    for source in &report.results {
        validated(source, &typed.read("expected.luau"));
    }

    assert_eq!(typed.bytes("input.luau"), expected);

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
    let mut requested = request(&fixture);
    requested.invocation.inputs = names.map(|name| Input::Path(fixture.path(name))).into();
    requested.overrides.formatting.indent_style = Some(IndentStyle::Spaces);
    requested.overrides.formatting.indent_width = NonZeroUsize::new(4);
    let report = format_report(&fixture, requested);
    assert_eq!(report.completion, Completion::Complete);
    assert_eq!(report.counts.selected, 3);
    assert_eq!(report.counts.unchanged, 0);
    assert_eq!(report.counts.changed, 3);
    assert_eq!(report.counts.written, 0);
    assert_eq!(report.counts.dependencies, 0);

    for (name, original) in names.into_iter().zip(originals) {
        let source = report
            .results
            .iter()
            .find(|source| {
                report
                    .snapshot
                    .origin(source.result.source.identity)
                    .and_then(|origin| origin.path())
                    == Some(fixture.path(name).as_path())
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
