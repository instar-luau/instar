//! Opt-in Roblox environments and instance-to-source mappings.

#[path = "../../instar-core/tests/support/roblox.rs"]
mod support;

use assert_cmd::Command;
use std::{error::Error, fs};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn update_is_exposed_and_preserves_standard_luau() -> TestResult {
    let directory = tempfile::tempdir()?;

    let help = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--help"])
        .assert()
        .success();

    assert!(String::from_utf8_lossy(&help.get_output().stdout).contains("--update"));

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--update", "-"])
        .current_dir(directory.path())
        .write_stdin("return 1")
        .assert()
        .success();

    Ok(())
}

#[test]
fn analyzes_instance_imports_from_sourcemaps_and_editor_input() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();

    fs::write(
        root.join("instar.toml"),
        "[roblox]\nsourcemap = 'sourcemap.json'",
    )?;

    fs::write(
        root.join("sourcemap.json"),
        r#"{"name":"Library","className":"Folder","children":[{"name":"Entry","className":"ModuleScript","filePaths":["entry.luau"]},{"name":"Main","className":"ModuleScript","filePaths":["main.luau"]}]}"#,
    )?;

    fs::write(root.join("entry.luau"), "--!strict\nreturn {Value = 1}")?;
    let main = root.join("main.luau");

    fs::write(
        &main,
        "--!strict\nlocal value: number = require(script.Parent.Entry).Value\nreturn value",
    )?;

    support::configure(root)?;

    for solver in ["new", "old"] {
        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--solver", solver])
            .arg(&main)
            .assert()
            .success();

        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--solver", solver, "--filename"])
            .arg(&main)
            .arg("-")
            .write_stdin(
                "--!strict\nlocal value: string = require(script.Parent.Entry).Value\nreturn value",
            )
            .assert()
            .code(1);

        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--mode", "nocheck", "--solver", solver, "--filename"])
            .arg(&main)
            .arg("-")
            .write_stdin("local callback = Instance.new('BindableFunction').OnInvoke\nInstance.new('Part').ClassName = 'Folder'\nreturn callback")
            .assert()
            .success();
    }

    Ok(())
}
