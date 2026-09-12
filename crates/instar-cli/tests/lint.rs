use assert_cmd::Command;
use std::{error::Error, fs};

type Result = std::result::Result<(), Box<dyn Error>>;

#[test]
fn rules_and_explanations_are_available_without_inputs() {
    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["lint", "--list"])
        .assert()
        .success()
        .get_output()
        .clone();

    assert_eq!(
        String::from_utf8(output.stdout)
            .expect("output")
            .lines()
            .count(),
        64
    );

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["lint", "--explain", "undefined_variable"])
        .assert()
        .success();

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["lint", "--explain", "missing"])
        .assert()
        .failure();
}

#[test]
fn lint_reports_levels_and_applies_safe_fixes() -> Result {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("source.luau");
    fs::write(&path, "local value = 1\nreturn 2\n")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("lint")
        .arg(&path)
        .assert()
        .success();

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["lint", "--fix"])
        .arg(&path)
        .assert()
        .success();

    assert_eq!(fs::read_to_string(&path)?, "local _value = 1\nreturn 2\n");
    fs::write(&path, "return unknown")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("lint")
        .arg(&path)
        .assert()
        .failure();

    fs::write(
        directory.path().join("instar.toml"),
        "[lint.rules]\nundefined_variable = 'allow'\n",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("lint")
        .arg(&path)
        .assert()
        .success();

    Ok(())
}

#[test]
fn named_input_is_fixed_without_writing_its_path() -> Result {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("source.luau");
    fs::write(&path, "return 3")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["lint", "--fix", "--filename"])
        .arg(&path)
        .arg("-")
        .write_stdin("local value = 1\nreturn 2\n")
        .assert()
        .success()
        .stdout("local _value = 1\nreturn 2\n");

    assert_eq!(fs::read_to_string(path)?, "return 3");

    Ok(())
}
