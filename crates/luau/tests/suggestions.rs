//! Native completion contexts, replacement ranges, and lexical loader identity.

use std::{io, time::Duration};

use instar_analysis::{Location, Options};

use instar_bridge::{
    Configuration,
    frontend::{Frontend, SuggestionContext, SuggestionKind},
};

use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    query: String,
    offset: usize,
    context: SuggestionContext,
    name: String,
    kind: SuggestionKind,
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

    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 11, &Configuration::new()?, &[])?;
    let options = Options::new(Duration::from_secs(5));

    for case in cases {
        let offset = source.find(&case.query).expect("query") + case.offset;
        let target = source.find(&case.target).expect("target") + case.target_offset;
        let suggestions = frontend.suggest("entry", offset, &options)?;

        assert_eq!(
            suggestions.location,
            Location {
                module: "entry".to_owned(),
                revision: 11,
                range: [target, target + case.length]
            }
        );

        assert_eq!(suggestions.context, case.context);

        let entry = suggestions
            .entries
            .iter()
            .find(|entry| entry.name == case.name)
            .expect("expected suggestion");

        assert_eq!(entry.kind, case.kind);
        assert_eq!(entry.callable, case.callable);
        assert!(entry.detail.contains(&case.detail), "{}", entry.detail);

        assert!(
            suggestions
                .entries
                .windows(2)
                .all(|entries| entries[0].name < entries[1].name)
        );
    }

    Ok(())
}

#[test]
fn native_suggestions_recover_after_incomplete_member_access() -> io::Result<()> {
    let source = include_str!("fixtures/suggestions/incomplete.luau");
    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 12, &Configuration::new()?, &[])?;
    let offset = source.trim_end().len();
    let suggestions = frontend.suggest("entry", offset, &Options::new(Duration::from_secs(5)))?;
    assert_eq!(suggestions.context, SuggestionContext::Property);
    assert_eq!(suggestions.location.range, [offset, offset]);
    assert_eq!(suggestions.index, Some([offset - 1, offset]));

    assert!(
        suggestions
            .entries
            .iter()
            .any(|entry| entry.name == "value" && entry.detail == "number")
    );

    Ok(())
}

#[test]
fn native_suggestions_distinguish_require_from_shadowed_functions() -> io::Result<()> {
    let mut frontend = Frontend::default();
    let configuration = Configuration::new()?;
    let options = Options::new(Duration::from_secs(5));

    for (source, revision, require) in [
        (include_str!("fixtures/suggestions/import.luau"), 13, true),
        (include_str!("fixtures/suggestions/shadow.luau"), 14, false),
        (include_str!("fixtures/suggestions/nested.luau"), 16, false),
    ] {
        frontend.insert("entry", source, revision, &configuration, &[])?;
        let start = source.find("\"./dependency\"").expect("string argument");
        let suggestions = frontend.suggest("entry", start + 4, &options)?;
        assert_eq!(suggestions.context, SuggestionContext::String);
        assert_eq!(suggestions.require, require);
        assert_eq!(suggestions.prefix.as_deref(), Some("./d"));

        assert_eq!(
            &source[suggestions.location.range[0]..suggestions.location.range[1]],
            "\"./dependency\""
        );
    }

    Ok(())
}

#[test]
fn native_suggestions_preserve_unterminated_import_prefixes() -> io::Result<()> {
    let source = include_str!("fixtures/suggestions/broken.luau");
    let mut frontend = Frontend::default();
    frontend.insert("entry", source, 15, &Configuration::new()?, &[])?;

    let suggestions =
        frontend.suggest("entry", source.len(), &Options::new(Duration::from_secs(5)))?;

    assert_eq!(suggestions.context, SuggestionContext::String);
    assert!(suggestions.require);
    assert_eq!(suggestions.prefix.as_deref(), Some("./dep"));

    assert_eq!(
        &source[suggestions.location.range[0]..suggestions.location.range[1]],
        "\"./dep"
    );

    Ok(())
}
