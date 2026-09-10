use std::{error::Error, fs};

use assert_cmd::Command;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn standard_input_and_output() -> TestResult {
    let directory = tempfile::tempdir()?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["format", "-"])
        .write_stdin("local  value=1")
        .assert()
        .success()
        .stdout("local value = 1\n");

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["format", "-", "--check"])
        .write_stdin("local value=1")
        .assert()
        .failure()
        .stdout("");

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["format", "-"])
        .write_stdin("local = =")
        .assert()
        .failure()
        .stdout("");

    Ok(())
}

#[test]
fn checks_without_writing_and_formats_in_place() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("main.luau"), "local value=1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "--check", "."])
        .assert()
        .failure()
        .stdout("");

    assert_eq!(fs::read_to_string(root.join("main.luau"))?, "local value=1");

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "--stdout", "main.luau"])
        .assert()
        .success()
        .stdout("local value = 1\n");

    assert_eq!(fs::read_to_string(root.join("main.luau"))?, "local value=1");

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success()
        .stdout("");

    assert_eq!(
        fs::read_to_string(root.join("main.luau"))?,
        "local value = 1\n"
    );

    let modified = fs::metadata(root.join("main.luau"))?.modified()?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .success();

    assert_eq!(fs::metadata(root.join("main.luau"))?.modified()?, modified);

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "--check", "."])
        .assert()
        .success()
        .stdout("");

    Ok(())
}

#[test]
fn invalid_files_remain_untouched() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::write(root.join("broken.luau"), "local = =")?;
    fs::write(root.join("valid.lua"), "return 1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "."])
        .assert()
        .failure();

    assert_eq!(fs::read_to_string(root.join("broken.luau"))?, "local = =");
    assert_eq!(fs::read_to_string(root.join("valid.lua"))?, "return 1\n");

    Ok(())
}

#[test]
fn configuration_follows_named_input() -> TestResult {
    let directory = tempfile::tempdir()?;
    let root = directory.path();
    fs::create_dir(root.join("project"))?;

    fs::write(
        root.join("project/instar.toml"),
        "[format]\nindent_type = 'spaces'\n",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "-", "--filename", "project/main.luau"])
        .write_stdin("do f() end")
        .assert()
        .success()
        .stdout("do\n    f()\nend\n");

    fs::write(
        root.join("project/instar.toml"),
        "[format]\nenabled = false\n",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["format", "-", "--config", "project/instar.toml"])
        .write_stdin("local = =")
        .assert()
        .success()
        .stdout("local = =");

    Ok(())
}
