//! Build planning, artifact publication, and incremental rebuild behavior.

use assert_cmd::Command;
use std::{
    error::Error,
    fs,
    io::{BufRead, BufReader},
    process::{Child, ChildStderr, Stdio},
};

type TestResult = Result<(), Box<dyn Error>>;

struct Watcher {
    child: Child,
    messages: BufReader<ChildStderr>,
}

impl Drop for Watcher {
    fn drop(&mut self) {
        drop(self.child.kill());
        drop(self.child.wait());
    }
}

impl Watcher {
    fn message(&mut self) -> std::result::Result<String, Box<dyn Error>> {
        let mut message = String::new();

        if self.messages.read_line(&mut message)? == 0 {
            return Err("watcher closed its output".into());
        }

        Ok(message)
    }
}

#[test]
fn watch_rebuilds_additions_removals_and_configuration_after_errors() -> TestResult {
    let directory = tempfile::tempdir()?;
    let configuration = directory.path().join("instar.toml");

    fs::write(
        &configuration,
        "[build]\ninputs = ['source']\noutput = 'output'\n",
    )?;

    fs::create_dir(directory.path().join("source"))?;
    let source = directory.path().join("source/main.luau");
    let output = directory.path().join("output/source/main.luau");
    fs::write(&source, "return 1")?;

    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["build", "--watch"])
        .arg(directory.path())
        .stderr(Stdio::piped())
        .spawn()?;

    let messages = BufReader::new(child.stderr.take().ok_or("watcher output")?);
    let mut watcher = Watcher { child, messages };
    assert!(watcher.message()?.starts_with("build:"));
    assert_eq!(fs::read_to_string(&output)?, "return 1");
    fs::write(&source, "local =")?;
    assert!(watcher.message()?.contains("stale"));
    assert_eq!(fs::read_to_string(&output)?, "return 1");
    fs::write(&source, "return 2")?;
    assert!(watcher.message()?.starts_with("build:"));
    assert_eq!(fs::read_to_string(&output)?, "return 2");
    fs::write(directory.path().join("source/added.luau"), "return 3")?;
    assert!(watcher.message()?.starts_with("build:"));
    assert!(directory.path().join("output/source/added.luau").exists());
    fs::remove_file(directory.path().join("source/added.luau"))?;
    assert!(watcher.message()?.contains("2 removed"));
    assert!(!directory.path().join("output/source/added.luau").exists());

    fs::write(
        &configuration,
        "[build]\ninputs = ['source']\noutput = 'output'\nminify = true\n",
    )?;

    assert!(watcher.message()?.starts_with("build:"));

    Ok(())
}

#[test]
fn plans_do_not_publish_and_builds_report_failures() -> TestResult {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        "[build]\ninputs = ['source']\noutput = 'output'\n",
    )?;

    fs::create_dir(directory.path().join("source"))?;
    fs::write(directory.path().join("source/main.luau"), "return 1")?;

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["build", "--plan"])
        .arg(directory.path())
        .assert()
        .success()
        .get_output()
        .clone();

    let text = String::from_utf8(output.stdout)?;
    assert!(text.contains("artifacts"));
    assert!(!directory.path().join("output").exists());

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("build")
        .arg(directory.path())
        .assert()
        .success();

    assert_eq!(
        fs::read_to_string(directory.path().join("output/source/main.luau"))?,
        "return 1"
    );

    fs::write(
        directory.path().join("source/main.luau"),
        "return require('./missing')",
    )?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("build")
        .arg(directory.path())
        .assert()
        .failure();

    assert_eq!(
        fs::read_to_string(directory.path().join("output/source/main.luau"))?,
        "return 1"
    );

    Ok(())
}

#[test]
fn profiles_select_bundle_destinations() -> TestResult {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        "[build]\nentry = 'main.luau'\nshape = 'bundle'\noutput = 'output/development.luau'\n[build.profiles.production]\noutput = 'output/production.luau'\nminify = true\n",
    )?;

    fs::write(directory.path().join("main.luau"), "return 1")?;

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["build", "--profile", "production"])
        .arg(directory.path())
        .assert()
        .success();

    assert!(directory.path().join("output/production.luau").is_file());
    assert!(!directory.path().join("output/development.luau").exists());

    Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["build", "--profile", "missing"])
        .arg(directory.path())
        .assert()
        .failure();

    Ok(())
}
