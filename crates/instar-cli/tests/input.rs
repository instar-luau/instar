//! Source traversal, standard input, and input deduplication.

use std::{error::Error, fs};

use assert_cmd::Command;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn input_requirements_are_shared() -> TestResult {
    let directory = tempfile::tempdir()?;

    for operation in ["analyze", "format"] {
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .arg(operation)
            .assert()
            .failure();

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args([operation, "--filename", "main.luau", "missing.luau"])
            .assert()
            .failure()
            .get_output()
            .clone();

        assert_eq!(
            String::from_utf8(output.stderr)?,
            format!("{operation}: --filename requires '-' input\n")
        );

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args([operation, "."])
            .assert()
            .success()
            .stdout("");
    }

    Ok(())
}

#[test]
fn unnamed_input_is_virtual_and_read_once() -> TestResult {
    let directory = tempfile::tempdir()?;

    for operation in ["analyze", "format"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args([operation, "-", "-"])
            .write_stdin("return 1")
            .assert()
            .success()
            .get_output()
            .clone();

        assert_eq!(
            output.stdout,
            if operation == "format" {
                b"return 1\n".as_slice()
            } else {
                b""
            }
        );

        assert!(!directory.path().join("stdin").exists());
    }

    Ok(())
}

#[test]
fn traversal_and_duplicate_inputs_are_shared() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("nested"))?;
    fs::write(root.join("nested/value.lua"), "return 1\n")?;
    fs::write(root.join("ignored.txt"), "not valid Luau")?;

    for operation in ["analyze", "format"] {
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args([operation, ".", "nested", "nested/value.lua"])
            .assert()
            .success()
            .stdout("");
    }

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "--stdout", "nested", "nested/value.lua"])
        .assert()
        .success()
        .stdout("return 1\n");

    Ok(())
}

#[test]
fn named_input_shadows_disk_without_writing_it() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("main.luau"), "return 1")?;

    for arguments in [
        vec!["format", "-", "main.luau", "--filename", "main.luau"],
        vec!["format", "main.luau", "-", "--filename", "main.luau"],
    ] {
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(arguments)
            .write_stdin("return 2")
            .assert()
            .success()
            .stdout("return 2\n");

        assert_eq!(fs::read_to_string(root.join("main.luau"))?, "return 1");
    }

    Ok(())
}

#[test]
fn mixed_sources_keep_output_and_writes_separate() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("main.luau"), "return 1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "main.luau", "-"])
        .write_stdin("return 2")
        .assert()
        .success()
        .stdout("return 2\n");

    assert_eq!(fs::read_to_string(root.join("main.luau"))?, "return 1\n");

    Ok(())
}
