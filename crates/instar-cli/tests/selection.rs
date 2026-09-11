use std::{error::Error, fs};

use assert_cmd::Command;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn analysis_uses_project_selection_and_preserves_explicit_inputs() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("generated"))?;
    let invalid = "local value: number = 'wrong'\nreturn value";
    fs::write(root.join("main.luau"), "return 1")?;

    for name in ["skip.luau", "keep.luau"] {
        fs::write(root.join("generated").join(name), invalid)?;
    }

    fs::write(
        root.join("instar.toml"),
        "mode = 'strict'\nexclude = ['generated']\ngrafts = {unused = 'missing.toml'}\n[format]\ninclude = ['generated']\nexclude = ['[']",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "."])
        .assert()
        .success();

    let result = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "generated/skip.luau"])
        .assert()
        .code(1);

    assert!(String::from_utf8_lossy(&result.get_output().stderr).contains("skip.luau"));

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "--filename", "generated/skip.luau", "-"])
        .write_stdin(invalid)
        .assert()
        .code(1);

    fs::write(
        root.join("instar.toml"),
        "mode = 'strict'\nexclude = ['generated']\ninclude = ['generated/keep.luau']\n[format]\nexclude = ['generated']",
    )?;

    let result = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "."])
        .assert()
        .code(1);

    assert!(String::from_utf8_lossy(&result.get_output().stderr).contains("keep.luau"));

    Ok(())
}

#[test]
fn analysis_checks_dependencies_excluded_from_entry_selection() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("generated"))?;

    fs::write(
        root.join("instar.toml"),
        "mode = 'strict'\nexclude = ['generated']",
    )?;

    fs::write(
        root.join("main.luau"),
        "return require('./generated/dependency')",
    )?;

    fs::write(
        root.join("generated/dependency.luau"),
        "local value: number = 'wrong'\nreturn value",
    )?;

    let result = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "."])
        .assert()
        .code(1);

    assert!(String::from_utf8_lossy(&result.get_output().stderr).contains("dependency.luau"));

    Ok(())
}

#[test]
fn analysis_inherits_selection_origins_and_empty_overrides() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("instar.toml"),
        "mode = 'strict'\nexclude = ['nested/*.luau']\ninclude = ['nested/keep.luau']",
    )?;

    for name in ["keep.luau", "skip.luau"] {
        fs::write(
            root.join("nested").join(name),
            "local value: number = 'wrong'\nreturn value",
        )?;
    }

    for (settings, status) in [
        ("", 1),
        ("include = []", 0),
        ("include = ['keep.luau']", 1),
        ("include = []\nexclude = []", 1),
    ] {
        fs::write(root.join("nested/instar.toml"), settings)?;

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["analyze", "."])
            .assert()
            .code(status);
    }

    Ok(())
}

#[test]
fn analysis_rejects_invalid_selection_globs() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "exclude = ['[']")?;
    fs::write(root.join("main.luau"), "return 1")?;

    let result = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "."])
        .assert()
        .failure();

    let message = String::from_utf8_lossy(&result.get_output().stderr);
    assert!(message.contains("instar.toml"));
    assert!(message.contains("invalid glob"));

    Ok(())
}

#[test]
fn walked_files_obey_selection_and_explicit_files_bypass_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("generated"))?;

    for name in ["generated/skip.luau", "generated/keep.luau", "main.luau"] {
        fs::write(root.join(name), "return 1")?;
    }

    fs::write(
        root.join("instar.toml"),
        "exclude = ['generated']\n[format]\ninclude = ['generated/keep.luau']\n",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(root.join("generated/skip.luau"))?,
        "return 1"
    );

    assert_eq!(
        fs::read_to_string(root.join("generated/keep.luau"))?,
        "return 1\n"
    );

    assert_eq!(fs::read_to_string(root.join("main.luau"))?, "return 1\n");

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "generated/skip.luau"])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(root.join("generated/skip.luau"))?,
        "return 1\n"
    );

    Ok(())
}

#[test]
fn format_selection_overrides_project_selection() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("keep.luau"), "return 1")?;
    fs::write(root.join("skip.luau"), "return 1")?;

    fs::write(
        root.join("instar.toml"),
        "exclude = ['*.luau']\ninclude = ['*.luau']\n[format]\nexclude = ['skip.luau']\n",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success();

    assert_eq!(fs::read_to_string(root.join("keep.luau"))?, "return 1\n");
    assert_eq!(fs::read_to_string(root.join("skip.luau"))?, "return 1");

    Ok(())
}

#[test]
fn invalid_globs_fail_before_writes() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("instar.toml"), "[format]\nexclude = ['[']\n")?;
    fs::write(root.join("main.luau"), "return 1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .failure();

    assert_eq!(fs::read_to_string(root.join("main.luau"))?, "return 1");

    Ok(())
}

#[test]
fn selection_lists_keep_their_configuration_origins() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;

    fs::write(
        root.join("instar.toml"),
        "[format]\nexclude = ['nested/*.luau']\n",
    )?;

    fs::write(
        root.join("nested/instar.toml"),
        "[format]\ninclude = ['keep.luau']\n",
    )?;

    fs::write(root.join("nested/keep.luau"), "return 1")?;
    fs::write(root.join("nested/skip.luau"), "return 1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(root.join("nested/keep.luau"))?,
        "return 1\n"
    );

    assert_eq!(
        fs::read_to_string(root.join("nested/skip.luau"))?,
        "return 1"
    );

    fs::write(root.join("nested/instar.toml"), "[format]\nexclude = []\n")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(root.join("nested/skip.luau"))?,
        "return 1\n"
    );

    Ok(())
}
