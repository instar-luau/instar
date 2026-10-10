//! Regression coverage for CLI input selection, deadlines, contexts and severities.

use std::{
    ffi::OsStr,
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
                "--source",
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
                .contains("requires --output")
        );

        assert_eq!(fs::read(&path)?, original);

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--color", "never", "format", "--write", "nested/entry.luau"])
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
        "[environment]\nroblox=false\n",
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
            assert_eq!(output.status.code(), Some(4), "{operation}: {output:?}");
            assert!(output.stdout.is_empty(), "{operation}: {output:?}");
            let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;

            assert!(
                error["error"]
                    .as_str()
                    .expect("JSON error")
                    .contains("deadline"),
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
    assert_eq!(diagnostics[0]["code"], "unused_variable");
    assert_eq!(diagnostics[0]["severity"], "info");

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--color", "never", "lint", "entry.luau"])
        .output()?;

    assert!(output.status.success(), "{output:?}");
    assert!(output.stdout.is_empty(), "{output:?}");
    let diagnostics = String::from_utf8_lossy(&output.stderr);

    assert!(
        diagnostics.contains("note[unused_variable]: binding is never read"),
        "{diagnostics}"
    );

    assert!(!diagnostics.contains("warning:"), "{diagnostics}");
    assert!(!diagnostics.contains("error:"), "{diagnostics}");

    directory.close()
}

#[test]
fn mapped_lint_visits_every_target_placement_and_analyzes_dependencies() -> io::Result<()> {
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

    let mut previous = None;

    for stdin in [false, false, true, true] {
        let output = if stdin {
            input(
                directory.path(),
                &["--json", "--stdin-filepath", "shared.luau", "lint", "-"],
                source.as_bytes(),
            )?
        } else {
            Command::new(env!("CARGO_BIN_EXE_instar"))
                .current_dir(directory.path())
                .args(["--json", "--color", "never", "lint"])
                .arg("shared.luau")
                .output()?
        };

        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        if let Some((previous_stdin, previous_report)) = &previous {
            if *previous_stdin == stdin {
                assert_eq!(&report, previous_report);
            } else {
                let stable_findings = |report: &serde_json::Value| {
                    report["diagnostics"]
                        .as_array()
                        .expect("diagnostics")
                        .iter()
                        .map(|diagnostic| {
                            let mut diagnostic = diagnostic.clone();
                            let record = diagnostic.as_object_mut().expect("diagnostic record");
                            drop(record.remove("identity"));
                            drop(record.remove("source"));

                            for related in record
                                .get_mut("related")
                                .expect("related locations")
                                .as_array_mut()
                                .expect("related locations")
                            {
                                drop(
                                    related
                                        .as_object_mut()
                                        .expect("related location")
                                        .remove("source"),
                                );
                            }

                            diagnostic
                        })
                        .collect::<Vec<_>>()
                };

                assert_eq!(stable_findings(&report), stable_findings(previous_report));
                assert_eq!(report["completion"], previous_report["completion"]);
                assert_eq!(report["counts"], previous_report["counts"]);
            }
        }

        previous = Some((stdin, report.clone()));

        assert_eq!(report["completion"], "Complete");
        let diagnostics = report["diagnostics"].as_array().expect("diagnostics");

        let findings = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic["code"] == "divide_by_zero")
            .collect::<Vec<_>>();

        assert_eq!(findings.len(), 2, "{report}");

        for (name, count) in [("shared.luau", 2), ("first.luau", 0), ("second.luau", 0)] {
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
    assert_eq!(diagnostics[0]["code"], "divide_by_zero");

    assert_eq!(
        Path::new(diagnostics[0]["path"].as_str().expect("diagnostic path")).file_name(),
        Some(OsStr::new("unmapped.luau")),
    );

    directory.close()
}

#[test]
fn jobs_are_a_validated_command_line_setting() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join("instar.toml"),
        "[environment]\nroblox=false\n",
    )?;

    fs::write(directory.path().join("entry.luau"), "return 1\n")?;

    for operation in ["lint", "format", "server", "check"] {
        let help = Command::new(env!("CARGO_BIN_EXE_instar"))
            .args([operation, "--jobs", "1", "--help"])
            .output()?;

        assert!(help.status.success(), "{help:?}");
        assert!(String::from_utf8_lossy(&help.stdout).contains("--jobs"));

        let invalid = Command::new(env!("CARGO_BIN_EXE_instar"))
            .args([operation, "--jobs", "0"])
            .output()?;

        assert_eq!(invalid.status.code(), Some(2), "{invalid:?}");
    }

    for operation in ["lint", "format", "check"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", operation, "--jobs", "1", "entry.luau"])
            .output()?;

        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["completion"], "Complete");
        assert_eq!(report["diagnostics"], serde_json::json!([]));
    }

    directory.close()
}

#[test]
fn configuration_print_is_rejected_without_contaminating_json() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(directory.path().join("entry.luau"), "return 1\n")?;

    for source in [
        "print('CONFIG_STDOUT')\nreturn {}\n",
        "local output = print\noutput('CONFIG_STDOUT')\nreturn {}\n",
        "_G.print('CONFIG_STDOUT')\nreturn {}\n",
    ] {
        fs::write(directory.path().join("instar.luau"), source)?;

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", "--color", "never", "check", "entry.luau"])
            .output()?;

        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;
        let message = error["error"].as_str().expect("JSON error");
        assert!(message.contains("instar.luau"), "{error}");
        assert!(message.contains("nil value"), "{error}");
    }

    directory.close()
}

#[test]
fn configured_stdout_flags_report_validation_errors() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    fs::write(
        directory.path().join(".luaurc"),
        "{\"languageMode\":\"strict\"}",
    )?;

    for (name, settings, source) in [
        (
            "DebugLuauLogSolver",
            "",
            "local value: number = 1\nreturn value\n",
        ),
        (
            "DebugLuauLogBindings",
            "",
            "local value: number = 1\nreturn value\n",
        ),
        (
            "DebugLuauLogSolverToJson",
            "LuauCyclicRequireTypeInference=false\n",
            "local value: number = 1\nreturn value\n",
        ),
        (
            "DebugLuauLogCFG",
            "DebugLuauCFG=true\nLuauCyclicRequireTypeInference=false\n",
            "local value: number = 1\nreturn value\n",
        ),
        (
            "DebugLuauDumpCFGJson",
            "DebugLuauCFG=true\nLuauCyclicRequireTypeInference=false\n",
            "local value: number = 1\nreturn value\n",
        ),
        (
            "DebugLuauLogTypeFamilies",
            "",
            "local function add<T>(value: T)\nreturn value + value\nend\nreturn add(1)\n",
        ),
        (
            "DebugLuauMagicTypes",
            "",
            "local value: _luau_print<number> = 1\nreturn value\n",
        ),
    ] {
        fs::write(
            directory.path().join("instar.toml"),
            format!("[environment]\nroblox=false\n[language.flags]\n{name}=true\n{settings}"),
        )?;

        fs::write(directory.path().join("entry.luau"), source)?;

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", "--color", "never", "check", "entry.luau"])
            .output()?;

        assert_eq!(output.status.code(), Some(2), "{name}: {output:?}");
        assert!(output.stdout.is_empty(), "{name}: {output:?}");
        let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;

        assert!(
            error["error"]
                .as_str()
                .expect("JSON error")
                .contains(&format!(
                    "Luau flag writes to stdout and is unavailable: {name}"
                )),
            "{name}: {error}"
        );
    }

    directory.close()
}

#[test]
fn cli_patterns_match_configuration_selection_and_preserve_literals() -> io::Result<()> {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../project/tests/fixtures/selection/cases.json"
    ))?;

    let directory = tempfile::tempdir()?;

    for source in fixture["sources"].as_array().expect("sources") {
        let path = directory.path().join(source.as_str().expect("source"));
        fs::create_dir_all(path.parent().expect("source parent"))?;
        fs::write(path, include_bytes!("fixtures/selection/original.luau"))?;
    }

    fs::write(
        directory.path().join("instar.toml"),
        "[environment]\nroblox=false",
    )?;

    fs::write(directory.path().join(".ignore"), "ignored.luau\n")?;

    fs::write(
        directory.path().join("ignored.luau"),
        include_bytes!("fixtures/selection/original.luau"),
    )?;

    for case in fixture["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|case| case["cli"] != false)
    {
        let pattern = case["pattern"].as_str().expect("pattern");

        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", "format", "--check", pattern])
            .output()?;

        assert_eq!(output.status.code(), Some(1), "{case}: {output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        let mut found = report["changed"]
            .as_array()
            .expect("changed sources")
            .iter()
            .map(|path| {
                Path::new(path.as_str().expect("path"))
                    .strip_prefix(directory.path())
                    .expect("relative source")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();

        let mut expected = case["expected"]
            .as_array()
            .expect("expected sources")
            .iter()
            .map(|source| source.as_str().expect("source").to_owned())
            .collect::<Vec<_>>();

        found.sort();
        expected.sort();
        assert_eq!(found, expected, "{case}: {report}");
    }

    for arguments in [
        vec!["entry.luau"],
        vec!["src"],
        vec!["src/"],
        vec!["entry.luau", "entry.luau"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", "format", "--check"])
            .args(&arguments)
            .output()?;

        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;

        assert_eq!(
            report["changed"].as_array().expect("changes").len(),
            if arguments[0].starts_with("src") {
                4
            } else {
                1
            }
        );
    }

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["--json", "format", "--check"])
        .arg(directory.path().join("entry.luau"))
        .output()?;

    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(report["changed"].as_array().expect("changes").len(), 1);

    for pattern in ["missing.luau", "*missing*", "[", "!src/**", "src/**x"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--json", "format", "--check", pattern])
            .output()?;

        assert_eq!(output.status.code(), Some(2), "{pattern}: {output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
        let error: serde_json::Value = serde_json::from_slice(&output.stderr)?;
        assert!(error["error"].is_string(), "{error}");
    }

    fs::write(
        directory.path().join("instar.toml"),
        "[selection]\nexclude=['entry.luau']\n[environment]\nroblox=false",
    )?;

    for operation in ["check", "lint"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(directory.path())
            .args(["--color", "never", operation, "entry.luau"])
            .output()?;

        assert!(output.status.success(), "{output:?}");

        assert!(
            String::from_utf8_lossy(&output.stderr).contains("1 skipped"),
            "{output:?}"
        );
    }

    directory.close()
}
