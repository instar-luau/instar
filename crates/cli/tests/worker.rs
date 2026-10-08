//! Worker execution from a standalone relocated CLI.

use std::{
    env::consts::EXE_SUFFIX,
    fs,
    io::{self, Write},
    process::{Command, Stdio},
    str,
};

#[test]
fn relocated_executable_runs_all_workers() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    let executable = directory.path().join(format!("tool{EXE_SUFFIX}"));

    fs::copy(env!("CARGO_BIN_EXE_instar"), &executable)?;

    fs::write(
        directory.path().join("instar.toml"),
        "[roblox]\nenabled=false\n",
    )?;

    fs::write(directory.path().join("entry.luau"), "return 1\n")?;

    for operation in ["format", "lint", "check"] {
        let mut command = Command::new(&executable);

        command
            .current_dir(directory.path())
            .args(["--json", "--color", "never", operation]);

        if operation == "format" {
            command.arg("--check");
        }

        let output = command.arg("entry.luau").output()?;
        assert!(output.status.success(), "{operation}: {output:?}");
        assert!(output.stderr.is_empty(), "{operation}: {output:?}");

        let report: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        assert_eq!(report["completion"], "Complete");
        assert_eq!(report["diagnostics"], serde_json::json!([]));
        assert_eq!(report["changed"], serde_json::json!([]));
    }

    directory.close()
}

#[test]
fn completions_exclude_internal_worker_modes() -> io::Result<()> {
    for shell in ["bash", "elvish", "fish", "nushell", "powershell", "zsh"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .args(["completions", shell])
            .output()?;

        assert!(output.status.success(), "{shell}: {output:?}");
        assert!(output.stderr.is_empty(), "{shell}: {output:?}");

        let script = str::from_utf8(&output.stdout).expect("UTF-8 completions");

        assert!(
            !script.contains("worker"),
            "{shell} exposes internal worker commands"
        );
    }

    Ok(())
}

#[test]
fn stdout_flag_is_rejected_by_worker_before_initialization() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args(["worker", "analysis"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let request = serde_json::json!({
        "sources": {},
        "declarations": {},
        "environments": {},
        "entries": [],
        "modules": [],
        "operation": "Parse",
        "timeout": {"secs": 5, "nanos": 0},
        "flags": {"DebugLuauLogSolver": true},
        "progress": false
    });

    let mut input = child.stdin.take().expect("piped stdin");
    serde_json::to_writer(&mut input, &request)?;
    input.write_all(b"\n")?;
    drop(input);
    let output = child.wait_with_output()?;

    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(response["Response"]["Err"]["kind"], "InvalidData");

    assert_eq!(
        response["Response"]["Err"]["message"],
        "Luau flag writes to stdout and is unavailable: DebugLuauLogSolver"
    );

    directory.close()
}
