//! Loading and resolving external Luau definition sources.

use instar_core::{
    analysis::{self, Options},
    lint,
    project::resolution::Resolver,
    source::SourceStore,
};

use std::{error::Error, fs};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn definitions_are_checked_and_loaded_in_order() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[analyze]\ndefinitions=['types.d.luau','globals.d.luau']",
    )?;

    fs::write(root.join("types.d.luau"), "export type Value = number")?;
    fs::write(root.join("globals.d.luau"), "declare value: Value")?;
    let main = root.join("main.luau");

    fs::write(
        &main,
        "--!strict\nlocal result: number = value\nreturn result",
    )?;

    for old_solver in [false, true] {
        let report = analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            std::slice::from_ref(&main),
            &Options {
                old_solver,
                ..Default::default()
            },
        )?;

        assert!(
            !report.has_errors(),
            "{:?}",
            report
                .diagnostics
                .iter()
                .map(|error| &error.message)
                .collect::<Vec<_>>()
        );
    }

    Ok(())
}

#[test]
fn build_constants_are_typed_analysis_and_lint_globals() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let main = root.join("main.luau");

    fs::write(
        root.join("instar.toml"),
        "[build.constants]\nDEBUG=false\nVERSION='development'\nLIMIT=1\n[lint.rules]\nundefined_variable='deny'",
    )?;

    fs::write(
        &main,
        "--!strict\nlocal debug: boolean = DEBUG\nlocal version: string = VERSION\nlocal limit: number = LIMIT\nreturn debug, version, limit",
    )?;

    assert!(
        !analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            std::slice::from_ref(&main),
            &Options::default(),
        )?
        .has_errors()
    );

    let lint = lint::analyze(
        &mut analysis::Session::default(),
        &mut SourceStore::default(),
        &main,
    )?;

    assert!(
        lint.findings
            .iter()
            .all(|finding| finding.rule != "undefined_variable")
    );

    fs::write(
        &main,
        "--!strict\nlocal limit: string = LIMIT\nreturn limit",
    )?;

    assert!(
        analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            &[main],
            &Options::default(),
        )?
        .has_errors()
    );

    Ok(())
}

#[test]
fn external_documentation_is_loaded_for_analyzed_sources() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let main = root.join("main.luau");
    let documentation = root.join("documentation.json");

    fs::write(
        root.join("instar.toml"),
        "[analyze]\ndocumentation=['documentation.json']",
    )?;

    fs::write(&main, "return 1")?;

    fs::write(
        &documentation,
        r#"{"@external/global/value":{"documentation":"External value."}}"#,
    )?;

    let report = analysis::analyze(
        &mut Resolver::new(&mut SourceStore::default()),
        std::slice::from_ref(&main),
        &Options::default(),
    )?;

    assert_eq!(
        report.documentation[&main]["@external/global/value"]["documentation"],
        "External value."
    );

    fs::write(&documentation, "[]")?;

    assert!(
        analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            &[main],
            &Options::default(),
        )
        .is_err()
    );

    Ok(())
}

#[test]
fn definition_diagnostics_keep_their_source_paths() -> TestResult {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("types.d.luau");

    fs::write(
        directory.path().join("instar.toml"),
        "[analyze]\ndefinitions=['types.d.luau']",
    )?;

    let main = directory.path().join("main.luau");
    fs::write(&main, "return 1")?;

    for (source, category) in [
        ("\ndeclare value: Missing", "TypeError"),
        ("\ndeclare value:", "SyntaxError"),
    ] {
        fs::write(&path, source)?;

        for entry in [&path, &main] {
            let report = analysis::analyze(
                &mut Resolver::new(&mut SourceStore::default()),
                std::slice::from_ref(entry),
                &Options::default(),
            )?;

            assert!(report.has_errors());

            assert!(report.diagnostics.iter().any(|error| error.path == path
                && error.line == 1
                && error.column > 0
                && error.message.starts_with(category)));
        }
    }

    Ok(())
}

#[test]
fn configured_definitions_use_editor_snapshots() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[analyze]\ndefinitions=['globals.luau']",
    )?;

    let main = root.join("main.luau");

    fs::write(
        &main,
        "--!strict\nlocal result: number = value\nreturn result",
    )?;

    let mut sources = SourceStore::default();
    let definition = sources.open(&root.join("globals.luau"), 1, "declare value: number")?;

    assert!(
        !analysis::analyze(
            &mut Resolver::new(&mut sources),
            std::slice::from_ref(&main),
            &Options::default()
        )?
        .has_errors()
    );

    sources.update(&definition, 2, "declare value: string")?;

    assert!(
        analysis::analyze(
            &mut Resolver::new(&mut sources),
            &[main],
            &Options::default()
        )?
        .has_errors()
    );

    Ok(())
}

#[test]
fn definition_environments_do_not_leak_between_entry_projects() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("instar.toml"),
        "[analyze]\ndefinitions=['globals.d.luau']",
    )?;

    fs::write(root.join("globals.d.luau"), "declare value: number")?;
    fs::write(root.join("nested/instar.toml"), "[analyze]\ndefinitions=[]")?;
    let main = root.join("main.luau");
    let nested = root.join("nested/main.luau");

    for path in [&main, &nested] {
        fs::write(path, "--!strict\nreturn value")?;
    }

    let report = analysis::analyze(
        &mut Resolver::new(&mut SourceStore::default()),
        &[main.clone(), nested.clone()],
        &Options::default(),
    )?;

    assert!(
        report
            .diagnostics
            .iter()
            .any(|error| error.path == nested && error.is_error)
    );

    assert!(
        !report
            .diagnostics
            .iter()
            .any(|error| error.path == main && error.is_error),
        "{:?}",
        report
            .diagnostics
            .iter()
            .map(|error| (&error.path, &error.message))
            .collect::<Vec<_>>()
    );

    Ok(())
}
