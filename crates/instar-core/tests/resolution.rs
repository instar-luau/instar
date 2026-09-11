use std::{error::Error, fs, sync::Arc};

use instar_core::{
    analysis::{self, Options},
    project::resolution::Resolver,
    source::SourceStore,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn resolves_files_directory_modules_and_dotted_names() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("package"))?;

    for (name, text) in [
        ("main.luau", "return 1"),
        ("value.luau", "return 2"),
        ("value.component.lua", "return 3"),
        ("package/init.luau", "return 4"),
    ] {
        fs::write(root.join(name), text)?;
    }

    let mut sources = SourceStore::default();
    let mut resolver = Resolver::new(&mut sources);

    for (specifier, filename) in [
        ("./value", "value.luau"),
        ("./value.component", "value.component.lua"),
        ("./package", "package/init.luau"),
    ] {
        assert_eq!(
            resolver.resolve(&root.join("main.luau"), specifier)?,
            Some(root.join(filename))
        );
    }

    assert_eq!(
        resolver.resolve(&root.join("package/init.luau"), "./value")?,
        Some(root.join("value.luau"))
    );

    assert_eq!(resolver.resolve(&root.join("main.luau"), "value")?, None);
    assert_eq!(resolver.resolve(&root.join("main.luau"), "./absent")?, None);

    Ok(())
}

#[test]
fn walks_namespace_segments_and_self_references() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("package"))?;

    for name in [
        "main.luau",
        "value.luau",
        "package/init.luau",
        "package/child.luau",
    ] {
        fs::write(root.join(name), "return 1")?;
    }

    let mut sources = SourceStore::default();
    let mut resolver = Resolver::new(&mut sources);

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "@self")?,
        Some(root.join("main.luau"))
    );

    assert_eq!(
        resolver.resolve(&root.join("package/init.luau"), "@self/child")?,
        Some(root.join("package/child.luau"))
    );

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), ".\\\\value")?,
        Some(root.join("value.luau"))
    );

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "./absent/../value")?,
        None
    );

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "./value/child")?,
        None
    );

    fs::write(root.join("package.luau"), "return 2")?;

    assert!(
        resolver
            .resolve(&root.join("main.luau"), "./package/child")
            .is_err()
    );

    Ok(())
}

#[test]
fn resolves_alias_chains_and_rejects_cycles() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;
    fs::write(root.join("value.luau"), "return 1")?;

    fs::write(
        root.join(".luaurc"),
        r#"{"aliases":{"first":"@second","second":"./value"}}"#,
    )?;

    let mut sources = SourceStore::default();

    assert_eq!(
        Resolver::new(&mut sources).resolve(&root.join("nested/main.luau"), "@first")?,
        Some(root.join("value.luau"))
    );

    fs::write(
        root.join("instar.toml"),
        "[aliases]\nfirst = '@second'\nsecond = '@first'\n",
    )?;

    let error = Resolver::new(&mut sources)
        .resolve(&root.join("main.luau"), "@first")
        .expect_err("alias cycle accepted");

    assert!(error.to_string().contains("cyclic alias"));

    Ok(())
}

#[test]
fn rejects_ambiguous_module_candidates() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("value.lua"), "return 1")?;
    fs::write(root.join("value.luau"), "return 2")?;
    let mut sources = SourceStore::default();

    assert!(
        Resolver::new(&mut sources)
            .resolve(&root.join("main.luau"), "./value")
            .is_err()
    );

    fs::remove_file(root.join("value.lua"))?;
    fs::create_dir(root.join("value"))?;

    assert!(
        Resolver::new(&mut sources)
            .resolve(&root.join("main.luau"), "./value")
            .is_err()
    );

    fs::remove_file(root.join("value.luau"))?;
    fs::write(root.join("value/init.lua"), "return 1")?;
    fs::write(root.join("value/init.luau"), "return 2")?;

    assert!(
        Resolver::new(&mut sources)
            .resolve(&root.join("main.luau"), "./value")
            .is_err()
    );

    Ok(())
}

#[test]
fn aliases_follow_proximity_then_format_and_preserve_origins() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join(".luaurc"),
        "{ // native parser accepts comments\n\"aliases\": {\"value\": \"./first\", \"parent\": \"./first\"}}",
    )?;

    fs::write(
        root.join("nested/.luaurc"),
        r#"{"aliases":{"value":"../second"}}"#,
    )?;

    for name in ["first", "second", "third"] {
        fs::write(root.join(format!("{name}.luau")), "return 1")?;
    }

    let from = root.join("nested/main.luau");
    let mut sources = SourceStore::default();
    let mut resolver = Resolver::new(&mut sources);

    assert_eq!(
        resolver.resolve(&from, "@VALUE")?,
        Some(root.join("second.luau"))
    );

    assert_eq!(
        resolver.resolve(&from, "@parent")?,
        Some(root.join("first.luau"))
    );

    fs::write(
        root.join("instar.toml"),
        "[aliases]\nvalue = './third'\ninherited = './third'\nchain = '@inherited'\n",
    )?;

    let mut resolver = Resolver::new(&mut sources);

    assert_eq!(
        resolver.resolve(&from, "@value")?,
        Some(root.join("second.luau"))
    );

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "@value")?,
        Some(root.join("third.luau"))
    );

    fs::write(
        root.join("nested/instar.toml"),
        "[aliases]\nvalue = '../first'\n",
    )?;

    let mut resolver = Resolver::new(&mut sources);

    for (specifier, filename) in [
        ("@VALUE", "first.luau"),
        ("@inherited", "third.luau"),
        ("@chain", "third.luau"),
    ] {
        assert_eq!(
            resolver.resolve(&from, specifier)?,
            Some(root.join(filename))
        );
    }

    assert_eq!(
        resolver.resolve(&from, "@parent")?,
        Some(root.join("first.luau"))
    );

    Ok(())
}

#[test]
fn source_and_configuration_snapshots_remain_consistent() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;
    fs::write(root.join(".luaurc"), r#"{"aliases":{"value":"./first"}}"#)?;
    fs::write(root.join("first.luau"), "return 1")?;
    fs::write(root.join("second.luau"), "return 2")?;
    let mut sources = SourceStore::default();
    let mut resolver = Resolver::new(&mut sources);
    let before = resolver.load(&root.join("first.luau"))?;

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "@value")?,
        Some(root.join("first.luau"))
    );

    fs::write(root.join("first.luau"), "return 'changed'")?;
    fs::write(root.join(".luaurc"), r#"{"aliases":{"value":"./second"}}"#)?;

    assert!(Arc::ptr_eq(
        &before,
        &resolver.load(&root.join("first.luau"))?
    ));

    assert_eq!(
        resolver.resolve(&root.join("nested/main.luau"), "@value")?,
        Some(root.join("first.luau"))
    );

    let mut resolver = Resolver::new(&mut sources);

    assert_eq!(
        resolver.resolve(&root.join("main.luau"), "@value")?,
        Some(root.join("second.luau"))
    );

    assert_ne!(
        before.revision(),
        resolver.load(&root.join("first.luau"))?.revision()
    );

    Ok(())
}

#[test]
fn native_analysis_uses_unsaved_modules_and_refreshes_between_operations() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[aliases]\nvalue = './value'\n")?;
    let mut sources = SourceStore::default();
    let main = root.join("main.luau");

    sources.open(
        &main,
        1,
        "--!strict\nlocal value: number = require('@value')\nreturn value\n",
    )?;

    let value = sources.open(&root.join("value.luau"), 1, "return 1")?;
    let nested = root.join("unsaved/child.luau");
    sources.open(&nested, 1, "return 2")?;

    assert_eq!(
        Resolver::new(&mut sources).resolve(&main, "./unsaved/child")?,
        Some(nested)
    );

    let report = analysis::analyze(
        &mut Resolver::new(&mut sources),
        std::slice::from_ref(&main),
        &Options::default(),
    )?;

    assert!(!report.has_errors());
    sources.update(&value, 2, "return 'wrong'")?;

    let report = analysis::analyze(
        &mut Resolver::new(&mut sources),
        std::slice::from_ref(&main),
        &Options::default(),
    )?;

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == main && diagnostic.message.contains("TypeError"))
    );

    assert!(!main.exists());
    assert!(!root.join("value.luau").exists());

    Ok(())
}

#[test]
fn native_analysis_preserves_dependency_types_for_diagnostics() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let mut sources = SourceStore::default();
    let main = root.join("main.luau");

    sources.open(
        &root.join("dependency.luau"),
        1,
        "local function create<Value>(value: Value): {read: () -> Value}\nreturn {read = function() return value end}\nend\nreturn create",
    )?;

    sources.open(
        &root.join("forward.luau"),
        1,
        "return require('./dependency')",
    )?;

    sources.open(
        &main,
        1,
        "local create = require('./forward')\nlocal value: string = create(1).read()\nreturn value",
    )?;

    let report = analysis::analyze(
        &mut Resolver::new(&mut sources),
        std::slice::from_ref(&main),
        &Options {
            strict: true,
            ..Options::default()
        },
    )?;

    assert!(report.annotations.is_empty());
    assert_eq!(report.diagnostics.len(), 1);
    let diagnostic = &report.diagnostics[0];
    assert_eq!(diagnostic.path, main);
    assert_eq!(diagnostic.line, 1);

    assert!(
        diagnostic.message.contains("number"),
        "{}",
        diagnostic.message
    );

    assert!(
        diagnostic.message.contains("string"),
        "{}",
        diagnostic.message
    );

    Ok(())
}

#[test]
fn native_analysis_reports_dependency_lint_errors() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join(".luaurc"), r#"{"lintErrors":true}"#)?;
    fs::write(root.join("value.luau"), "local unused = 1\nreturn 1")?;
    fs::write(root.join("main.luau"), "return require('./value')")?;
    let mut sources = SourceStore::default();

    let report = analysis::analyze(
        &mut Resolver::new(&mut sources),
        &[root.join("main.luau")],
        &Options::default(),
    )?;

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == root.join("value.luau")
                && diagnostic.is_error
                && diagnostic.message.contains("LocalUnused"))
    );

    Ok(())
}

#[test]
fn executable_configuration_is_rejected_without_evaluation() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("main.luau"), "return 1")?;

    fs::write(
        directory.path().join(".config.luau"),
        "error('must not execute')",
    )?;

    let mut sources = SourceStore::default();

    let result = analysis::analyze(
        &mut Resolver::new(&mut sources),
        &[directory.path().join("main.luau")],
        &Options::default(),
    );

    let Err(error) = result else {
        return Err("executable configuration accepted".into());
    };

    assert!(
        error
            .to_string()
            .contains("executable configuration is not supported")
    );

    Ok(())
}

#[test]
fn invalid_project_configuration_is_an_error_even_without_alias_imports() -> TestResult {
    let directory = tempfile::tempdir()?;
    fs::write(directory.path().join("main.luau"), "return 1")?;
    fs::write(directory.path().join("instar.toml"), "unknown = 1")?;
    let mut sources = SourceStore::default();

    let result = analysis::analyze(
        &mut Resolver::new(&mut sources),
        &[directory.path().join("main.luau")],
        &Options::default(),
    );

    assert!(result.is_err());

    Ok(())
}
