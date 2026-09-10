use instar_core::{
    analysis::{self, Options},
    resolution::Resolver,
    source::SourceStore,
};
use std::{error::Error, fs};

type Result = std::result::Result<(), Box<dyn Error>>;

#[test]
fn definitions_are_checked_and_loaded_in_order() -> Result {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "definitions=['types.d.luau','globals.d.luau']",
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
fn definition_diagnostics_keep_their_source_paths() -> Result {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("types.d.luau");

    fs::write(
        directory.path().join("instar.toml"),
        "definitions=['types.d.luau']",
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
fn configured_definitions_use_editor_snapshots() -> Result {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "definitions=['globals.luau']")?;
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
fn definition_environments_do_not_leak_between_entry_projects() -> Result {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;
    fs::write(root.join("instar.toml"), "definitions=['globals.d.luau']")?;
    fs::write(root.join("globals.d.luau"), "declare value: number")?;
    fs::write(root.join("nested/instar.toml"), "definitions=[]")?;
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
            .any(|error| error.path == main && error.is_error)
    );

    Ok(())
}
