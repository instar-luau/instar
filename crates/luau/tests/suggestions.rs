//! Native completion contexts, replacement ranges, and lexical loader identity.

use std::{io, time::Duration};

use instar_luau::{
    AnalysisPurpose, CompletionContext, CompletionKind, ImportIdentity, ImportOutcome,
    ImportRequest, ImportSite, Native, PositionInput, ResolvedImport,
};

use instar_shared::{Location, Span};
use serde::Deserialize;

use crate::support::{location, module, options, request};

#[derive(Deserialize)]
struct Case {
    query: String,
    offset: usize,
    context: CompletionContext,
    name: String,
    kind: CompletionKind,
    detail: String,
    callable: bool,
    target: String,
    target_offset: usize,
    length: usize,
}

#[test]
fn native_suggestions_preserve_context_types_and_replacement_ranges() -> io::Result<()> {
    let source = include_str!("fixtures/suggestions/input.luau");

    let cases: Vec<Case> =
        serde_json::from_str(include_str!("fixtures/suggestions/expected.json"))?;

    let mut input = request(vec![module(1, source, 11)?])?;
    input.purpose = AnalysisPurpose::Editor;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let reference = established.modules[0]
        .reference
        .clone()
        .expect("editor reference");

    for case in cases {
        let offset = source.find(&case.query).expect("query") + case.offset;
        let target = source.find(&case.target).expect("target") + case.target_offset;

        let suggestions = native.complete(
            &PositionInput {
                analysis: reference.clone(),
                offset,
            },
            &options(Duration::from_secs(5)),
        )?;

        assert_eq!(
            suggestions.replacement,
            Location::new(
                &input.modules[0].source.source,
                Span {
                    start: target,
                    end: target + case.length
                }
            )?
        );

        assert_eq!(suggestions.context, case.context);

        let entry = suggestions
            .candidates
            .iter()
            .find(|entry| entry.name == case.name)
            .expect("expected suggestion");

        assert_eq!(entry.kind, case.kind);
        assert_eq!(entry.callable, case.callable);
        assert!(entry.detail.contains(&case.detail), "{}", entry.detail);

        assert!(
            suggestions
                .candidates
                .windows(2)
                .all(|entries| entries[0].name < entries[1].name)
        );
    }

    native.finish()
}

#[test]
fn native_suggestions_recover_after_incomplete_member_access() -> io::Result<()> {
    let source = include_str!("fixtures/suggestions/incomplete.luau");
    let mut input = request(vec![module(1, source, 12)?])?;
    input.purpose = AnalysisPurpose::Editor;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;
    let offset = source.trim_end().len();

    let suggestions = native.complete(
        &PositionInput {
            analysis: established.modules[0]
                .reference
                .clone()
                .expect("editor reference"),
            offset,
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(suggestions.context, CompletionContext::Property);

    assert_eq!(
        suggestions.replacement.span,
        Span {
            start: offset,
            end: offset
        }
    );

    assert_eq!(
        suggestions.operator.map(|location| location.span),
        Some(Span {
            start: offset - 1,
            end: offset
        })
    );

    assert!(
        suggestions
            .candidates
            .iter()
            .any(|entry| entry.name == "value" && entry.detail == "number")
    );

    native.finish()
}

#[test]
fn native_suggestions_distinguish_require_from_shadowed_functions() -> io::Result<()> {
    let mut native = Native::default();

    for (source, revision, require) in [
        (include_str!("fixtures/suggestions/import.luau"), 13, true),
        (include_str!("fixtures/suggestions/shadow.luau"), 14, false),
        (include_str!("fixtures/suggestions/nested.luau"), 16, false),
    ] {
        let mut entry = module(1, source, revision)?;
        let start = source.find("\"./dependency\"").expect("string argument");

        if revision != 14 {
            let call = source.find("require(").expect("require call");

            let argument = if require {
                [start, start + "\"./dependency\"".len()]
            } else {
                [
                    source.find("string.format").expect("argument"),
                    source.trim_end().len() - 1,
                ]
            };

            entry.imports.push(ResolvedImport {
                site: ImportSite {
                    identity: ImportIdentity(1),
                    call: location(&entry, [call, source.trim_end().len()])?,
                    argument: location(&entry, argument)?,
                    request: if require {
                        ImportRequest::Static("./dependency".to_owned())
                    } else {
                        ImportRequest::Nonconstant
                    },
                },
                outcome: if require {
                    ImportOutcome::Unresolved
                } else {
                    ImportOutcome::Nonconstant
                },
            });
        }

        let mut input = request(vec![entry])?;
        input.purpose = AnalysisPurpose::Editor;
        let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

        let suggestions = native.complete(
            &PositionInput {
                analysis: established.modules[0]
                    .reference
                    .clone()
                    .expect("editor reference"),
                offset: start + 4,
            },
            &options(Duration::from_secs(5)),
        )?;

        assert_eq!(suggestions.context, CompletionContext::String);
        assert_eq!(suggestions.require, require);
        assert_eq!(suggestions.prefix.as_deref(), Some("./d"));

        assert_eq!(
            &source[suggestions.replacement.span.start..suggestions.replacement.span.end],
            "\"./dependency\""
        );
    }

    native.finish()
}

#[test]
fn native_suggestions_preserve_unterminated_import_prefixes() -> io::Result<()> {
    let source = include_str!("fixtures/suggestions/broken.luau");
    let mut entry = module(1, source, 15)?;

    entry.imports.push(ResolvedImport {
        site: ImportSite {
            identity: ImportIdentity(1),
            call: location(
                &entry,
                [source.find("require(").expect("call"), source.len()],
            )?,
            argument: location(
                &entry,
                [source.find('"').expect("broken string"), source.len()],
            )?,
            request: ImportRequest::Nonconstant,
        },
        outcome: ImportOutcome::Nonconstant,
    });

    let mut input = request(vec![entry])?;
    input.purpose = AnalysisPurpose::Editor;
    let mut native = Native::default();
    let established = native.analyze(&input, &options(Duration::from_secs(5)))?;

    let suggestions = native.complete(
        &PositionInput {
            analysis: established.modules[0]
                .reference
                .clone()
                .expect("editor reference"),
            offset: source.len(),
        },
        &options(Duration::from_secs(5)),
    )?;

    assert_eq!(suggestions.context, CompletionContext::String);
    assert!(suggestions.require);
    assert_eq!(suggestions.prefix.as_deref(), Some("./dep"));

    assert_eq!(
        &source[suggestions.replacement.span.start..suggestions.replacement.span.end],
        "\"./dep"
    );

    native.finish()
}
