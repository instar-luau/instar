//! Worker execution from a standalone relocated CLI.

use std::{fs, io, process::Command};

#[test]
fn relocated_executable_runs_all_workers() -> io::Result<()> {
    let directory = tempfile::tempdir()?;

    let executable = directory
        .path()
        .join(format!("tool{}", std::env::consts::EXE_SUFFIX));

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

        let script = std::str::from_utf8(&output.stdout).expect("UTF-8 completions");

        assert!(
            !script.contains("worker"),
            "{shell} exposes internal worker commands"
        );
    }

    Ok(())
}
