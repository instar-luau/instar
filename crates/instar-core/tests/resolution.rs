use std::{error::Error, fs, sync::Arc};

use instar_core::{
    analysis::{self, Options},
    project::resolution::Resolver,
    source::SourceStore,
};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn index_queries_preserve_symbols_calls_and_dependencies() -> TestResult {
    let directory = tempfile::tempdir()?;
    let main = directory.path().join("main.luau");

    fs::write(
        directory.path().join("dependency.luau"),
        "local function calculate(value: number): number return value + 1 end\nreturn table.freeze({ calculate = calculate })",
    )?;

    let mut sources = SourceStore::default();
    let mut session = analysis::Session::default();
    let mut source = sources.open(&main, 1, "local dependency = require('./dependency')\nlocal function run(value: number): number return dependency.calculate(value) end\nreturn run(1)")?;

    let identity = |entry: &analysis::EditorEntry| {
        (
            entry.name.clone(),
            entry.path.clone(),
            entry.range,
            entry.selection,
            entry.kind,
            entry.declaration,
            entry.modifiers,
            entry.caller,
            entry.container,
        )
    };

    for version in [1, 2] {
        if version == 2 {
            source = sources.update(
                &source,
                version,
                "local dependency = require('./dependency')\nreturn dependency.calculate(2)",
            )?;

            session.change(&main);
        }

        let mut expected = Vec::new();
        let mut indexed = Vec::new();

        for operation in ["index", "symbols", "calls", "links"] {
            let report = session.query(
                &mut sources,
                std::slice::from_ref(&main),
                &main,
                line_index::LineCol { line: 0, col: 0 },
                operation,
            )?;

            assert!(!report.has_errors());

            let Some(analysis::EditorResult::Entries(entries)) = report.editor else {
                return Err("editor entries".into());
            };

            assert!(!entries.is_empty(), "{operation}");

            match operation {
                "index" => indexed.extend(entries.iter().map(identity)),

                "symbols" => {
                    assert!(
                        entries
                            .iter()
                            .all(|entry| entry.caller.is_none() && entry.kind != Some(3))
                    );

                    expected.extend(entries.iter().map(identity));
                }

                "calls" => {
                    assert!(entries.iter().all(|entry| entry.caller.is_some()));
                    expected.extend(entries.iter().map(identity));
                }

                "links" => {
                    assert!(
                        entries
                            .iter()
                            .all(|entry| entry.kind == Some(3) && entry.caller.is_none())
                    );

                    expected.extend(entries.iter().map(identity));
                }

                _ => unreachable!(),
            }
        }

        expected.sort();
        indexed.sort();
        assert_eq!(indexed, expected);
    }

    Ok(())
}

#[test]
fn sessions_refresh_sources_dependencies_and_configuration() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let main = root.join("main.luau");
    let dependency = root.join("value.luau");
    let configuration = root.join("instar.toml");
    let source = "local value: number = require('@value')\nreturn value";
    let settings = "mode = 'strict'\naliases = {value = './value'}";
    let mut session = analysis::Session::default();

    for old_solver in [false, true] {
        let mut sources = SourceStore::default();
        let opened = sources.open(&main, 1, source)?;
        let modules = std::slice::from_ref(&main);

        let options = Options {
            old_solver,
            annotations: true,
            ..Default::default()
        };

        fs::write(&configuration, settings)?;
        fs::write(&dependency, "return 1")?;

        for _ in 0..2 {
            let report = session.analyze(&mut sources, modules, &options)?;
            assert!(!report.has_errors());

            assert!(
                report
                    .annotations
                    .iter()
                    .any(|annotation| annotation.path == main)
            );
        }

        fs::write(&dependency, "return 'changed'")?;

        assert!(
            session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        fs::write(
            &configuration,
            "mode = 'nocheck'\naliases = {value = './value'}",
        )?;

        assert!(
            !session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        fs::write(&configuration, settings)?;

        assert!(
            session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        let updated = sources.update(
            &opened,
            2,
            "local value: string = require('@value')\nreturn value",
        )?;

        assert!(
            !session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        fs::remove_file(&dependency)?;

        assert!(
            session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        fs::write(&dependency, "return 'restored'")?;

        assert!(
            !session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        fs::write(
            &configuration,
            "mode = 'strict'\naliases = {value = './other'}",
        )?;

        fs::write(root.join("other.luau"), "return 1")?;

        assert!(
            session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );

        sources.update(&updated, 3, source)?;

        assert!(
            !session
                .analyze(&mut sources, modules, &options)?
                .has_errors()
        );
    }

    Ok(())
}

#[test]
fn sessions_refresh_checking_modes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let main = root.join("main.luau");
    fs::write(root.join("instar.toml"), "mode = 'nocheck'")?;
    fs::write(&main, "local value: number = 'wrong'\nreturn value")?;
    let mut session = analysis::Session::default();
    let mut sources = SourceStore::default();

    for old_solver in [false, true] {
        for (mode, errors) in [
            (Some(analysis::Mode::Strict), true),
            (None, false),
            (Some(analysis::Mode::Nocheck), false),
            (Some(analysis::Mode::Strict), true),
        ] {
            let report = session.analyze(
                &mut sources,
                std::slice::from_ref(&main),
                &Options {
                    mode,
                    old_solver,
                    ..Default::default()
                },
            )?;

            assert_eq!(report.has_errors(), errors);
        }
    }

    Ok(())
}

#[test]
fn sessions_reload_definitions_and_repeat_their_diagnostics() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    let main = root.join("main.luau");
    let definitions = root.join("types.d.luau");

    fs::write(
        root.join("instar.toml"),
        "mode = 'strict'\ndefinitions = ['types.d.luau']",
    )?;

    fs::write(&main, "local value: number = application\nreturn value")?;
    let mut session = analysis::Session::default();
    let mut sources = SourceStore::default();

    for old_solver in [false, true] {
        let options = Options {
            old_solver,
            ..Default::default()
        };

        for (declaration, errors) in [
            ("declare application: number", false),
            ("declare application: string", true),
            ("declare application: number", false),
            ("declare application:", true),
        ] {
            fs::write(&definitions, declaration)?;

            for _ in 0..2 {
                let report =
                    session.analyze(&mut sources, std::slice::from_ref(&main), &options)?;

                assert_eq!(report.has_errors(), errors);

                if declaration.ends_with(':') {
                    assert!(
                        report
                            .diagnostics
                            .iter()
                            .any(|diagnostic| diagnostic.path == definitions)
                    );
                }
            }
        }

        fs::remove_file(&definitions)?;

        assert!(
            session
                .analyze(&mut sources, std::slice::from_ref(&main), &options)
                .is_err()
        );

        fs::write(&definitions, "declare application: number")?;

        assert!(
            !session
                .analyze(&mut sources, std::slice::from_ref(&main), &options)?
                .has_errors()
        );
    }

    Ok(())
}

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
            mode: Some(analysis::Mode::Strict),
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
fn executable_configuration_is_loaded_and_reports_its_path() -> TestResult {
    let directory = tempfile::tempdir()?;
    let config = directory.path().join(".config.luau");
    fs::write(&config, "return {luau = 1}")?;
    let main = directory.path().join("main.luau");
    fs::write(&main, "local value: string = 1\nreturn value")?;
    let mut sources = SourceStore::default();

    let report = analysis::analyze(
        &mut Resolver::new(&mut sources),
        std::slice::from_ref(&main),
        &Options::default(),
    )?;

    assert!(report.has_errors());

    assert!(
        report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.path == config)
    );

    Ok(())
}

#[test]
fn upstream_configuration_candidates_are_ambiguous() -> TestResult {
    let directory = tempfile::tempdir()?;
    let main = directory.path().join("main.luau");
    fs::write(&main, "return 1")?;

    for (first, second) in [
        (".config.luau", "config.luau"),
        (".luaurc", ".config.luau"),
        (".luaurc", "config.luau"),
    ] {
        fs::write(directory.path().join(first), "{}")?;
        fs::write(directory.path().join(second), "{}")?;

        let result = analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            std::slice::from_ref(&main),
            &Options::default(),
        );

        assert!(result.err().is_some_and(|error| {
            error
                .to_string()
                .contains("ambiguous upstream configuration")
        }));

        fs::remove_file(directory.path().join(first))?;
        fs::remove_file(directory.path().join(second))?;
    }

    Ok(())
}

#[test]
fn executable_aliases_use_shared_resolution_and_instar_precedence() -> TestResult {
    for filename in [".config.luau", "config.luau"] {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        fs::create_dir(root.join("nested"))?;

        fs::write(
            root.join(filename),
            "return {luau = {aliases = {Entries = './value'}}}",
        )?;

        fs::write(root.join("value.luau"), "return 1")?;
        fs::write(root.join("replacement.luau"), "return 'value'")?;
        let main = root.join("nested/main.luau");

        fs::write(
            &main,
            "--!strict\nlocal value: number = require('@ENTRIES')\nreturn value",
        )?;

        assert_eq!(
            Resolver::new(&mut SourceStore::default()).resolve(&main, "@entries")?,
            Some(root.join("value.luau"))
        );

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
                    .map(|diagnostic| &diagnostic.message)
                    .collect::<Vec<_>>()
            );
        }

        fs::write(
            root.join("instar.toml"),
            "[aliases]\nentries = './replacement'",
        )?;

        assert_eq!(
            Resolver::new(&mut SourceStore::default()).resolve(&main, "@ENTRIES")?,
            Some(root.join("replacement.luau"))
        );

        let report = analysis::analyze(
            &mut Resolver::new(&mut SourceStore::default()),
            &[main],
            &Options::default(),
        )?;

        assert!(
            report.has_errors(),
            "Instar alias must override upstream alias"
        );
    }

    Ok(())
}

#[test]
fn executable_settings_inherit_and_override_upstream_configuration() -> TestResult {
    for filename in [".config.luau", "config.luau"] {
        let directory = tempfile::tempdir()?;
        let root = directory.path();
        fs::create_dir(root.join("nested"))?;

        fs::write(
            root.join(".luaurc"),
            r#"{"languageMode":"strict","aliases":{"entry":"./value"}}"#,
        )?;

        fs::write(root.join("value.luau"), "return 1")?;
        let path = root.join("nested/main.luau");

        fs::write(
            &path,
            "local value: string = require('@entry')\nreturn value",
        )?;

        for old_solver in [false, true] {
            let options = Options {
                old_solver,
                ..Default::default()
            };

            fs::write(
                root.join("nested").join(filename),
                "return {luau = {lint = {['*'] = false}}}",
            )?;

            let report = analysis::analyze(
                &mut Resolver::new(&mut SourceStore::default()),
                std::slice::from_ref(&path),
                &options,
            )?;

            assert!(report.has_errors());

            fs::write(
                root.join("nested").join(filename),
                "return {luau = {languagemode = 'nocheck'}}",
            )?;

            let report = analysis::analyze(
                &mut Resolver::new(&mut SourceStore::default()),
                std::slice::from_ref(&path),
                &options,
            )?;

            assert!(!report.has_errors());
        }
    }

    Ok(())
}

#[test]
fn configuration_types_are_local_and_validate_returns() -> TestResult {
    for filename in [".config.luau", "config.luau"] {
        for old_solver in [false, true] {
            let directory = tempfile::tempdir()?;
            let path = directory.path().join(filename);

            fs::write(
                &path,
                "--!strict\nlocal mode: LanguageMode = 'strict'\nlocal warning: LintWarning = 'LocalUnused'\nlocal options: LuauConfig = {languagemode = mode, lint = {[warning] = false}}\nlocal settings: Config = {luau = options}\nreturn settings",
            )?;

            let options = Options {
                old_solver,
                ..Default::default()
            };

            let report = analysis::analyze(
                &mut Resolver::new(&mut SourceStore::default()),
                std::slice::from_ref(&path),
                &options,
            )?;

            assert!(
                !report.has_errors(),
                "{:?}",
                report
                    .diagnostics
                    .iter()
                    .map(|diagnostic| &diagnostic.message)
                    .collect::<Vec<_>>()
            );

            let main = directory.path().join("main.luau");

            fs::write(
                &main,
                "--!strict\nlocal settings: Config = {}\nreturn settings",
            )?;

            let report = analysis::analyze(
                &mut Resolver::new(&mut SourceStore::default()),
                &[main],
                &options,
            )?;

            assert!(
                report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains("Unknown type 'Config'")),
                "{:?}",
                report
                    .diagnostics
                    .iter()
                    .map(|diagnostic| &diagnostic.message)
                    .collect::<Vec<_>>()
            );

            for (source, message) in [
                (
                    "--!strict\nreturn {luau = {languagemode = 'invalid'}}",
                    "TypeError:",
                ),
                (
                    "--!strict\nlocal warning: LintWarning = 'InvalidWarning'\nreturn {luau = {lint = {[warning] = true}}}",
                    "TypeError:",
                ),
                (
                    "--!strict\nreturn {luau = {lint = {InvalidWarning = true}}}",
                    "Unknown lint InvalidWarning",
                ),
            ] {
                fs::write(&path, source)?;

                let report = analysis::analyze(
                    &mut Resolver::new(&mut SourceStore::default()),
                    std::slice::from_ref(&path),
                    &options,
                )?;

                assert!(
                    report
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.path == path
                            && diagnostic.message.starts_with(message)),
                    "{:?}",
                    report
                        .diagnostics
                        .iter()
                        .map(|diagnostic| &diagnostic.message)
                        .collect::<Vec<_>>()
                );
            }
        }
    }

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
