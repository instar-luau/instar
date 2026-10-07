//! Regression coverage for CLI input selection, deadlines, contexts and severities.

use std::{
    fs,
    io::{self, Write},
    path::Path,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

fn input(directory: &Path, arguments: &[&str], source: &[u8]) -> io::Result<Output> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory)
        .args(["--color", "never"])
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child.stdin.take().expect("piped stdin").write_all(source)?;

    child.wait_with_output()
}

#[test]
fn skipped_stdin_preserves_original_source_and_selection_counts() -> io::Result<()> {
    let source = include_bytes!("fixtures/selection/input.luau");
    let original = include_bytes!("fixtures/selection/original.luau");

    for configuration in [
        include_str!("fixtures/selection/include.toml"),
        include_str!("fixtures/selection/exclude.toml"),
    ] {
        let directory = tempfile::tempdir()?;
        fs::write(directory.path().join("instar.toml"), configuration)?;
        let nested = directory.path().join("nested");
        fs::create_dir(&nested)?;

        fs::write(
            nested.join("instar.toml"),
            include_str!("fixtures/selection/nested.toml"),
        )?;

        let path = nested.join("entry.luau");
        fs::write(&path, original)?;

        let output = input(
            directory.path(),
            &["--stdin-filepath", "nested/entry.luau", "format", "-"],
            source,
        )?;

        assert!(output.status.success(), "{output:?}");
        assert_eq!(output.stdout, source);
        let summary = String::from_utf8_lossy(&output.stderr);

        assert!(
            summary.contains("1 file, 0 formatted, 0 unchanged, 0 unsuccessful, 1 skipped"),
            "{summary}"
        );

        assert!(summary.contains("0 diagnostics"), "{summary}");
        assert_eq!(fs::read(&path)?, original);

        let output = input(
            directory.path(),
            &[
                "--json",
                "--stdin-filepath",
                "nested/entry.luau",
                "format",
                "--check",
                "-",
            ],
            source,
        )?;

        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["completion"], "Complete");
        assert_eq!(report["diagnostics"], serde_json::json!([]));
        assert_eq!(report["changed"], serde_json::json!([]));
        assert_eq!(fs::read(&path)?, original);

        let output = input(
            directory.path(),
            &[
                "--json",
                "--stdin-filepath",
                "nested/entry.luau",
                "format",
                "-",
            ],
            source,
        )?;

        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;

        assert!(
            error["error"]
                .as_str()
                .expect("JSON error")
                .contains("requires --check")
        );

        assert_eq!(fs::read(&path)?, original);

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--color", "never", "format", "nested/entry.luau"])
            .output()?;

        assert!(output.status.success(), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        assert_eq!(fs::read(&path)?, original);
        directory.close()?;
    }

    Ok(())
}

#[test]
fn stdin_deadline_exits_without_waiting_for_pipe_eof() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        "[roblox]\nenabled=false\n",
    )?;

    for operation in ["format", "check", "lint"] {
        for partial in [false, true] {
            let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
                .current_dir(directory.path())
                .args([
                    "--json",
                    "--color",
                    "never",
                    "--timeout",
                    "1",
                    operation,
                    "-",
                ])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;

            let mut writer = child.stdin.take().expect("piped stdin");

            if partial {
                writer.write_all(b"return ")?;
            }

            let started = Instant::now();
            let mut expired = false;

            while child.try_wait()?.is_none() {
                if started.elapsed() >= Duration::from_secs(3) {
                    expired = true;
                    child.kill()?;
                    break;
                }

                thread::sleep(Duration::from_millis(10));
            }

            drop(writer);
            let output = child.wait_with_output()?;
            assert!(!expired, "{operation} waited for pipe EOF: {output:?}");
            assert_eq!(output.status.code(), Some(2), "{operation}: {output:?}");
            assert!(output.stdout.is_empty(), "{operation}: {output:?}");
            let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;

            assert!(
                error["error"]
                    .as_str()
                    .expect("JSON error")
                    .contains("Timeout"),
                "{operation}: {error}"
            );
        }
    }

    directory.close()
}

#[test]
fn informational_lint_retains_severity_and_success() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/information/instar.toml"),
    )?;

    fs::write(
        directory.path().join(".luaurc"),
        include_str!("fixtures/.luaurc"),
    )?;

    fs::write(
        directory.path().join("entry.luau"),
        include_str!("fixtures/information/entry.luau"),
    )?;

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--json", "--color", "never", "lint", "entry.luau"])
        .output()?;

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["completion"], "Complete");
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{report}");
    assert_eq!(diagnostics[0]["kind"], "Rule(UnusedVariable)");
    assert_eq!(diagnostics[0]["severity"], "info");

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--color", "never", "lint", "entry.luau"])
        .output()?;

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostics = String::from_utf8_lossy(&output.stderr);

    assert!(
        diagnostics.contains("note: binding is never read"),
        "{diagnostics}"
    );

    assert!(!diagnostics.contains("warning:"), "{diagnostics}");
    assert!(!diagnostics.contains("error:"), "{diagnostics}");

    directory.close()
}

#[test]
fn mapped_lint_visits_every_placement_and_its_dependencies() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let source = include_str!("fixtures/mapping/shared.luau");

    fs::write(
        directory.path().join("instar.toml"),
        include_str!("fixtures/mapping/instar.toml"),
    )?;

    fs::write(
        directory.path().join(".luaurc"),
        include_str!("fixtures/.luaurc"),
    )?;

    fs::write(directory.path().join("shared.luau"), source)?;

    fs::write(
        directory.path().join("first.luau"),
        include_str!("fixtures/mapping/first.luau"),
    )?;

    fs::write(
        directory.path().join("second.luau"),
        include_str!("fixtures/mapping/second.luau"),
    )?;

    fs::write(
        directory.path().join("unmapped.luau"),
        include_str!("fixtures/mapping/unmapped.luau"),
    )?;

    fs::write(
        directory.path().join("sourcemap.json"),
        include_str!("fixtures/mapping/sourcemap.json"),
    )?;

    for stdin in [false, true] {
        let output = if stdin {
            input(
                directory.path(),
                &["--json", "--stdin-filepath", "shared.luau", "lint", "-"],
                source.as_bytes(),
            )?
        } else {
            Command::new(env!("CARGO_BIN_EXE_instar"))
                .current_dir(directory.path())
                .args(["--json", "--color", "never", "lint", "shared.luau"])
                .output()?
        };

        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["completion"], "Complete");
        let diagnostics = report["diagnostics"].as_array().expect("diagnostics");

        let findings = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["kind"] == "Rule(DivideByZero)")
            .collect::<Vec<_>>();

        assert_eq!(findings.len(), 4, "{report}");

        for (name, count) in [("shared.luau", 2), ("first.luau", 1), ("second.luau", 1)] {
            assert_eq!(
                findings
                    .iter()
                    .filter(|finding| {
                        Path::new(finding["path"].as_str().expect("diagnostic path"))
                            .file_name()
                            .is_some_and(|file| file == name)
                    })
                    .count(),
                count,
                "{report}",
            );
        }
    }

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--json", "--color", "never", "lint", "unmapped.luau"])
        .output()?;

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["completion"], "Complete");
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{report}");
    assert_eq!(diagnostics[0]["kind"], "Rule(DivideByZero)");

    assert_eq!(
        Path::new(diagnostics[0]["path"].as_str().expect("diagnostic path")).file_name(),
        Some(std::ffi::OsStr::new("unmapped.luau")),
    );

    directory.close()
}
