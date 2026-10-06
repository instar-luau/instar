//! Native lint warnings, semantic facts and shared-session boundaries.

use instar_bridge::{
    Configuration,
    frontend::{Definition, FactKind, Frontend, Site},
};

use instar_check::{Completion, Kind, Options, Reason};
use std::{io, path::Path, time::Duration};

fn options() -> Options {
    Options::new(Duration::from_secs(5))
}

fn configuration(source: &str, path: &str) -> io::Result<Configuration> {
    let mut configuration = Configuration::new()?;
    configuration.apply(source, Path::new(path), Duration::from_secs(2))?;

    Ok(configuration)
}

#[test]
fn warning_codes_fatal_bits_and_utf8_ranges_preserve_checker_results() -> io::Result<()> {
    let configuration = configuration(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true},"lintErrors":true}"#,
        ".luaurc",
    )?;

    let mut frontend = Frontend::new()?;
    let source = "-- π\r\nlocal unused = 1\r\nreturn 2";
    frontend.insert("entry", source, 8, &configuration, &[])?;
    let entries = ["entry".to_owned()];
    let before = frontend.check(&entries, &options())?;
    let linted = frontend.lint(&entries, &options())?;
    assert_eq!(linted.completion, Completion::Complete);
    assert_eq!(linted.diagnostics, Vec::new());
    assert_eq!(linted.facts, Vec::new());
    assert_eq!(linted.warnings.len(), 1);
    let warning = &linted.warnings[0];
    assert_eq!(warning.code, 7);
    assert_eq!(warning.name, "LocalUnused");
    assert!(warning.fatal);
    assert_ne!(warning.message, "");
    assert_eq!(warning.location.module, "entry");
    assert_eq!(warning.location.revision, 8);

    assert_eq!(
        &source[warning.location.range[0]..warning.location.range[1]],
        "unused"
    );

    assert_eq!(frontend.check(&entries, &options())?, before);

    Ok(())
}

#[test]
fn executable_layers_directives_and_reconfiguration_control_warnings() -> io::Result<()> {
    let enabled = configuration(
        "return {luau={lint={['*']=false,LocalUnused=true},linterrors=true}}",
        ".config.luau",
    )?;

    let disabled = configuration("return {luau={lint={['*']=false}}}", ".config.luau")?;
    let mut frontend = Frontend::new()?;
    let entries = ["entry".to_owned()];
    frontend.insert("entry", "local unused = 1", 1, &enabled, &[])?;
    let warning = frontend.lint(&entries, &options())?;
    assert_eq!(warning.warnings.len(), 1);
    assert!(warning.warnings[0].fatal);

    frontend.insert(
        "entry",
        "--!nolint LocalUnused\nlocal unused = 1",
        2,
        &enabled,
        &[],
    )?;

    assert_eq!(frontend.lint(&entries, &options())?.warnings, Vec::new());
    frontend.insert("entry", "local unused = 1", 3, &disabled, &[])?;
    assert_eq!(frontend.lint(&entries, &options())?.warnings, Vec::new());
    frontend.insert("entry", "local unused = 1", 4, &enabled, &[])?;
    let restored = frontend.lint(&entries, &options())?;
    assert_eq!(restored.warnings.len(), 1);
    assert_eq!(restored.warnings[0].location.revision, 4);

    Ok(())
}

#[test]
fn typed_native_warnings_use_the_shared_type_graph() -> io::Result<()> {
    let configuration = configuration(
        r#"{"languageMode":"strict","lint":{"*":false,"TableOperations":true}}"#,
        ".luaurc",
    )?;

    let mut frontend = Frontend::new()?;
    let source = "local record = {field = 1}\nreturn #record";
    frontend.insert("entry", source, 5, &configuration, &[])?;
    let entries = ["entry".to_owned()];
    let checked = frontend.check(&entries, &options())?;
    let linted = frontend.lint(&entries, &options())?;
    assert_eq!(linted.completion, Completion::Complete);

    let warning = linted
        .warnings
        .iter()
        .find(|warning| warning.name == "TableOperations")
        .expect("typed table operation warning");

    assert_eq!(warning.code, 23);
    assert!(!warning.fatal);

    assert_eq!(
        &source[warning.location.range[0]..warning.location.range[1]],
        "#record"
    );

    assert_eq!(frontend.check(&entries, &options())?, checked);

    Ok(())
}

#[test]
fn host_dependency_contexts_retain_their_warning_revisions() -> io::Result<()> {
    let configuration = configuration(r#"{"lint":{"*":false,"LocalUnused":true}}"#, ".luaurc")?;
    let mut frontend = Frontend::new()?;
    let source = "local loader = require\nreturn loader('./absent')";
    let call = source.find("loader(").expect("call");
    let argument = source.find("'./").expect("argument");

    frontend.insert(
        "entry",
        source,
        2,
        &configuration,
        &[Site {
            call: [call, source.len()],
            argument: [argument, source.len() - 1],
            static_request: true,
            target: Some("contextual".to_owned()),
        }],
    )?;

    frontend.insert(
        "contextual",
        "--!nocheck\nlocal unused = 1\nreturn 3",
        9,
        &configuration,
        &[],
    )?;

    let linted = frontend.lint(&["entry".to_owned()], &options())?;
    assert_eq!(linted.completion, Completion::Complete);
    assert_eq!(linted.modules, ["contextual", "entry"]);
    assert_eq!(linted.warnings.len(), 1);
    assert_eq!(linted.warnings[0].location.module, "contextual");
    assert_eq!(linted.warnings[0].location.revision, 9);

    assert_eq!(
        frontend
            .lint_semantic(&["entry".to_owned()], &["entry".to_owned()], &options())?
            .completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn invalid_definitions_retain_available_syntax_warnings_and_diagnostics() -> io::Result<()> {
    let configuration = configuration(
        r#"{"lint":{"*":false,"LocalUnused":true,"TableLiteral":true}}"#,
        ".luaurc",
    )?;

    let mut frontend = Frontend::new()?;

    frontend.insert(
        "entry",
        "type Row = {field: number, field: number}\nlocal unused = 1\nreturn 2",
        1,
        &configuration,
        &[],
    )?;

    frontend.definitions(
        "entry",
        &[Definition {
            name: "broken".to_owned(),
            revision: 3,
            text: "declare broken:".to_owned(),
        }],
    )?;

    let linted = frontend.lint(&["entry".to_owned()], &options())?;

    assert_eq!(
        linted.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(linted.diagnostics.iter().any(|diagnostic| {
        diagnostic.location.module == "broken"
            && diagnostic.location.revision == 3
            && matches!(diagnostic.kind, Kind::Syntax { .. })
    }));

    assert!(
        linted
            .warnings
            .iter()
            .any(|warning| warning.name == "LocalUnused")
    );

    assert!(
        linted
            .warnings
            .iter()
            .any(|warning| warning.name == "TableLiteral")
    );

    frontend.definitions("entry", &[])?;

    assert_eq!(
        frontend.lint(&["entry".to_owned()], &options())?.completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn inferred_any_facts_exclude_annotations_and_inferred_numbers() -> io::Result<()> {
    let configuration = Configuration::new()?;
    let mut frontend = Frontend::new()?;
    let source = "--!strict\nlocal explicit: any = 1\nlocal inferred = explicit\nlocal known = 1\nlocal callback: (any) -> any = function(parameter) return parameter end\nreturn inferred, known, callback";
    frontend.insert("entry", source, 6, &configuration, &[])?;
    let entries = ["entry".to_owned()];
    let checked = frontend.check(&entries, &options())?;
    let linted = frontend.lint_semantic(&entries, &entries, &options())?;
    assert_eq!(linted.completion, Completion::Complete);
    assert_eq!(linted.facts.len(), 2);

    assert!(linted.facts.iter().any(|fact| {
        fact.kind == FactKind::ImplicitAnyLocal
            && &source[fact.location.range[0]..fact.location.range[1]] == "inferred"
    }));

    assert!(linted.facts.iter().any(|fact| {
        fact.kind == FactKind::ImplicitAnyParameter
            && &source[fact.location.range[0]..fact.location.range[1]] == "parameter"
    }));

    assert!(linted.facts.iter().all(|fact| fact.location.revision == 6));
    assert_eq!(frontend.check(&entries, &options())?, checked);

    Ok(())
}

#[test]
fn nocheck_and_interruption_have_explicit_completion_without_stale_warnings() -> io::Result<()> {
    let configuration = configuration(r#"{"lint":{"*":false,"LocalUnused":true}}"#, ".luaurc")?;
    let mut frontend = Frontend::new()?;

    frontend.insert(
        "entry",
        "--!nocheck\nlocal unused = 1",
        1,
        &configuration,
        &[],
    )?;

    let entries = ["entry".to_owned()];

    assert_eq!(
        frontend.lint(&entries, &options())?.completion,
        Completion::Complete
    );

    let semantic = frontend.lint_semantic(&entries, &entries, &options())?;

    assert_eq!(
        semantic.completion,
        Completion::Incomplete(Reason::Analysis)
    );

    assert_eq!(semantic.facts, Vec::new());

    assert!(semantic.diagnostics.iter().any(|diagnostic| {
        matches!(diagnostic.kind, Kind::Analysis { code: None })
            && diagnostic.message.contains("nocheck")
    }));

    let cancelled = options();
    cancelled.cancellation.cancel();

    assert_eq!(
        frontend.lint(&entries, &cancelled)?.completion,
        Completion::Incomplete(Reason::Cancelled)
    );

    assert_eq!(
        frontend
            .lint(&entries, &Options::new(Duration::ZERO))?
            .completion,
        Completion::Incomplete(Reason::Timeout)
    );

    frontend.insert("entry", "return 1", 2, &configuration, &[])?;
    let retry = frontend.lint(&entries, &options())?;
    assert_eq!(retry.completion, Completion::Complete);
    assert_eq!(retry.warnings, Vec::new());

    Ok(())
}

#[test]
fn syntax_diagnostics_are_retained_without_leaking_source_type_errors() -> io::Result<()> {
    let configuration = configuration(
        r#"{"languageMode":"strict","lint":{"*":false,"TableOperations":true}}"#,
        ".luaurc",
    )?;

    let mut frontend = Frontend::new()?;
    let entries = ["entry".to_owned()];

    frontend.insert(
        "entry",
        "local number: number = 'bad'\nreturn number",
        1,
        &configuration,
        &[],
    )?;

    assert!(
        frontend
            .check(&entries, &options())?
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Type { .. }))
    );

    assert_eq!(frontend.lint(&entries, &options())?.diagnostics, Vec::new());
    frontend.insert("entry", "local = 1", 2, &configuration, &[])?;
    let malformed = frontend.lint(&entries, &options())?;

    assert!(malformed.diagnostics.iter().any(|diagnostic| {
        diagnostic.location.module == "entry"
            && diagnostic.location.revision == 2
            && matches!(diagnostic.kind, Kind::Syntax { .. })
    }));

    assert_eq!(
        frontend
            .lint_semantic(&entries, &["unavailable".to_owned()], &options())
            .expect_err("semantic module must be reachable")
            .kind(),
        io::ErrorKind::InvalidInput
    );

    Ok(())
}
