//! Command availability, diagnostic separation, and exit statuses.

use std::{fs, path::Path, process::Command};

fn invoke(command: &str, path: &Path, expected: i32) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .args(["--plain", command])
        .arg(path)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(expected), "{output:?}");

    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn command_surface() {
    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();

    for command in ["check", "lint", "format", "lsp"] {
        assert!(help.contains(&format!("  {command} ")), "{help}");
    }

    for command in ["check", "lint"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .arg(command)
            .output()
            .unwrap();

        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn diagnostic_separation_and_severities() {
    let directory = std::env::temp_dir().join(format!("commands-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let types = directory.join("types.luau");
    let warnings = directory.join("warnings.luau");
    let syntax = directory.join("syntax.luau");

    fs::write(
        &types,
        "--!strict\nlocal value: number = \"text\"\nreturn value\n",
    )
    .unwrap();

    fs::write(&warnings, "local unused = 1\nif true then end\nreturn 1\n").unwrap();
    fs::write(&syntax, "local =\n").unwrap();

    for (native_errors, level, expected) in [
        (false, "allow", 0),
        (false, "info", 0),
        (false, "warn", 0),
        (false, "deny", 1),
        (true, "allow", 1),
    ] {
        fs::write(
            directory.join("instar.toml"),
            format!("[roblox]\nenabled = false\n[lint]\nlint_errors = {native_errors}\n[lint.luau]\n\"*\" = false\nLocalUnused = true\n[lint.rules]\nempty_if = \"{level}\"\n"),
        ).unwrap();

        let checked = invoke("check", &types, 1);
        assert_ne!(checked, "");
        assert!(!checked.contains("LocalUnused:") && !checked.contains("empty_if:"));
        assert_eq!(invoke("lint", &types, 0), "");
        assert_eq!(invoke("check", &warnings, 0), "");
        let linted = invoke("lint", &warnings, expected);

        let severity = if native_errors { "error" } else { "warning" };

        assert!(
            linted.contains(&format!("{severity}: LocalUnused:")),
            "{linted}"
        );

        if level == "allow" {
            assert!(!linted.contains("empty_if:"));
        } else {
            let severity = match level {
                "info" => "info",
                "warn" => "warning",
                _ => "error",
            };

            assert!(
                linted.contains(&format!("{severity}: empty_if:")),
                "{linted}"
            );
        }
    }

    for command in ["check", "lint"] {
        assert_ne!(invoke(command, &syntax, 1), "");
        invoke(command, &directory.join("missing.luau"), 2);
    }

    let declarations = directory.join("globals.d.luau");
    fs::write(&declarations, "declare value: MissingType\n").unwrap();

    for definitions in ["", "[luau.definitions]\n\"@custom\" = \"globals.d.luau\"\n"] {
        fs::write(
            directory.join("instar.toml"),
            format!("[roblox]\nenabled = false\n{definitions}"),
        )
        .unwrap();

        assert_ne!(invoke("check", &declarations, 1), "");
        assert_eq!(invoke("lint", &declarations, 2), "");
    }

    fs::write(directory.join("instar.toml"), "invalid = [").unwrap();

    for command in ["check", "lint"] {
        invoke(command, &types, 2);
    }

    fs::remove_dir_all(directory).unwrap();
}
