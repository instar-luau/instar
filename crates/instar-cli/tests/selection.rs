use std::{error::Error, fs};

use assert_cmd::Command;

type TestResult = Result<(), Box<dyn Error>>;

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
