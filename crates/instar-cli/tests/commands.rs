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

    fs::write(
        directory.join(".luaurc"),
        r#"{"lint":{"*":false},"lintErrors":true}"#,
    )
    .unwrap();

    for (native_level, level, expected) in [
        ("warn", "allow", 0),
        ("info", "info", 0),
        ("allow", "warn", 0),
        ("warn", "deny", 1),
        ("deny", "allow", 1),
    ] {
        fs::write(
            directory.join("instar.toml"),
            format!("[roblox]\nenabled = false\n[lint.local_unused]\nlevel = \"{native_level}\"\n[lint.empty_if]\nlevel = \"{level}\"\n"),
        ).unwrap();

        let linted = invoke("lint", &warnings, expected);

        for (rule, level) in [("LocalUnused", native_level), ("empty_if", level)] {
            if level == "allow" {
                assert!(!linted.contains(&format!("{rule}:")), "{linted}");
            } else {
                let severity = match level {
                    "info" => "info",
                    "warn" => "warning",
                    _ => "error",
                };

                assert!(linted.contains(&format!("{severity}: {rule}:")), "{linted}");
            }
        }
    }

    let checked = invoke("check", &types, 1);
    assert_ne!(checked, "");
    assert!(!checked.contains("LocalUnused:") && !checked.contains("empty_if:"));
    assert_eq!(invoke("lint", &types, 0), "");
    assert_eq!(invoke("check", &warnings, 0), "");

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

#[test]
fn operations_resolve_nested_filter_overrides() {
    let directory = std::env::temp_dir().join(format!("command-filters-{}", std::process::id()));
    let selected = directory.join("selected");
    fs::create_dir_all(&selected).unwrap();

    fs::write(
        directory.join("instar.toml"),
        "include = [\"selected/**\"]\nexclude = [\"selected/**\"]\n[roblox]\nenabled = false\n[lsp.index]\nexclude = [\"**\"]\n",
    )
    .unwrap();

    fs::write(
        selected.join("instar.toml"),
        "[check]\nexclude = []\n[lint]\nexclude = []\n[lint.empty_if]\nlevel = \"deny\"\n[format]\nexclude = []\n",
    )
    .unwrap();

    fs::write(
        selected.join("checked.luau"),
        "--!strict\nlocal value:number=\"text\"\nreturn value\n",
    )
    .unwrap();

    fs::write(selected.join("linted.luau"), "if true then end\n").unwrap();
    fs::write(directory.join("hidden.luau"), "local =\n").unwrap();

    let checked = invoke("check", &directory, 1);
    assert!(checked.contains("checked.luau"), "{checked}");
    assert!(!checked.contains("empty_if:") && !checked.contains("hidden.luau"));
    let linted = invoke("lint", &directory, 1);
    assert!(linted.contains("error: empty_if:"), "{linted}");
    assert!(!linted.contains("hidden.luau"));
    invoke("format", &directory, 0);

    assert_eq!(
        fs::read_to_string(selected.join("checked.luau")).unwrap(),
        "--!strict\nlocal value: number = \"text\"\nreturn value\n"
    );

    assert_eq!(
        fs::read_to_string(directory.join("hidden.luau")).unwrap(),
        "local =\n"
    );

    fs::remove_dir_all(directory).unwrap();
}
