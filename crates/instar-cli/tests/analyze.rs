use std::fs;

use assert_cmd::Command;

#[test]
fn analyzes_original_bytes_and_reports_type_and_syntax_errors() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.luau");

    for (bytes, expected) in [
        (&b""[..], None),
        (&b"return '\xff'\n"[..], None),
        (&b"local value: number = 'wrong'\nreturn value\n"[..], None),
        (
            &b"--!strict\nlocal value: number = 'wrong'\nreturn value\n"[..],
            Some("TypeError"),
        ),
        (&b"local =\n"[..], Some("SyntaxError")),
    ] {
        fs::write(&source, bytes).unwrap();

        let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .arg("analyze")
            .arg(&source)
            .assert();

        if let Some(expected) = expected {
            let assertion = assertion.code(1);
            let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
            assert!(stderr.contains(expected), "{stderr}");
            assert!(stderr.contains("source.luau"), "{stderr}");
        } else {
            assertion.success().stdout("").stderr("");
        }
    }
}

#[test]
fn analyzes_original_standard_input_bytes() {
    for (source, status) in [
        (&b"return '\xff'\n"[..], 0),
        (
            &b"--!strict\nlocal value: number = 'wrong'\nreturn value\n"[..],
            1,
        ),
        (&b"local =\n"[..], 1),
    ] {
        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "-"])
            .write_stdin(source)
            .assert()
            .code(status);
    }
}

#[test]
fn analyzes_directories_and_resolves_modules_with_upstream_configuration() {
    let directory = tempfile::tempdir().unwrap();

    fs::write(
        directory.path().join(".luaurc"),
        r#"{"languageMode":"strict"}"#,
    )
    .unwrap();

    fs::write(directory.path().join("value.luau"), "return 'wrong'\n").unwrap();

    fs::write(
        directory.path().join("main.luau"),
        "local value: number = require('./value')\nreturn value\n",
    )
    .unwrap();

    for path in [
        directory.path().to_path_buf(),
        directory.path().join("main.luau"),
    ] {
        let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .arg("analyze")
            .arg(path)
            .assert()
            .code(1);

        let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
        assert!(stderr.contains("TypeError"), "{stderr}");
        assert!(stderr.contains("main.luau"), "{stderr}");
    }
}

#[test]
fn handles_missing_files_and_requires_inputs() {
    let directory = tempfile::tempdir().unwrap();

    let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .arg("analyze")
        .arg(directory.path().join("missing.luau"))
        .assert()
        .code(1);

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(stderr.contains("missing.luau"), "{stderr}");

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .arg("analyze")
        .assert()
        .code(2);
}

#[test]
fn analyzes_multiple_files_and_filenames_that_look_like_options() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("-source.luau"), "return 1\n").unwrap();
    fs::write(directory.path().join("second.luau"), "return 2\n").unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .current_dir(directory.path())
        .args(["analyze", "--", "-source.luau", "second.luau"])
        .assert()
        .success()
        .stderr("");
}

#[test]
fn honors_aliases_and_reports_invalid_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let configuration = directory.path().join(".luaurc");

    fs::write(
        &configuration,
        r#"{"languageMode":"strict","aliases":{"value":"./value"}}"#,
    )
    .unwrap();

    fs::write(directory.path().join("value.luau"), "return 42\n").unwrap();
    let source = directory.path().join("source.luau");

    fs::write(
        &source,
        "local value: number = require('@value')\nreturn value\n",
    )
    .unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .arg("analyze")
        .arg(&source)
        .assert()
        .success()
        .stderr("");

    fs::write(&configuration, r#"{"languageMode":"invalid"}"#).unwrap();

    let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .arg("analyze")
        .arg(&source)
        .assert()
        .code(1);

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(stderr.contains(".luaurc"), "{stderr}");
}

#[test]
fn executable_configuration_controls_analysis_and_has_local_types() {
    for filename in [".config.luau", "config.luau"] {
        let directory = tempfile::tempdir().unwrap();
        let configuration = directory.path().join(filename);
        fs::write(&configuration, "local settings: Config = {luau = {languagemode = 'strict', aliases = {entry = './value'}}}\nreturn settings").unwrap();
        fs::write(directory.path().join("value.luau"), "return 1").unwrap();
        let source = directory.path().join("main.luau");

        fs::write(
            &source,
            "local value: number = require('@ENTRY')\nreturn value",
        )
        .unwrap();

        for solver in ["new", "old"] {
            Command::new(assert_cmd::cargo::cargo_bin!("instar"))
                .args(["analyze", "--solver", solver])
                .arg(&configuration)
                .arg(&source)
                .assert()
                .success()
                .stderr("");
        }

        fs::write(
            &source,
            "local value: string = require('@entry')\nreturn value",
        )
        .unwrap();

        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .arg("analyze")
            .arg(&source)
            .assert()
            .code(1);
    }
}

#[test]
fn explicit_strict_mode_reports_type_errors() {
    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--mode=strict", "-"])
        .write_stdin("local value: number = 'wrong'\nreturn value\n")
        .assert()
        .code(1);
}

#[test]
fn checking_modes_respect_configuration_and_directives() {
    let directory = tempfile::tempdir().unwrap();

    for solver in ["new", "old"] {
        for (mode, source, status) in [
            (
                "strict",
                "local function field(value) return value.name end\nreturn field(1)",
                1,
            ),
            (
                "nonstrict",
                "local function field(value) return value.name end\nreturn field(1)",
                0,
            ),
            (
                "nonstrict",
                "local value: number = 'wrong'\nreturn value",
                i32::from(solver == "old"),
            ),
            ("nocheck", "local value: number = 'wrong'\nreturn value", 0),
            (
                "nocheck",
                "--!strict\nlocal value: number = 'wrong'\nreturn value",
                1,
            ),
            (
                "strict",
                "--!nocheck\nlocal value: number = 'wrong'\nreturn value",
                0,
            ),
            (
                "strict",
                "--!nonstrict\nlocal function field(value) return value.name end\nreturn field(1)",
                0,
            ),
            ("nocheck", "local =", 1),
        ] {
            Command::new(assert_cmd::cargo::cargo_bin!("instar"))
                .current_dir(directory.path())
                .args(["analyze", "--mode", mode, "--solver", solver, "-"])
                .write_stdin(source)
                .assert()
                .code(status);
        }
    }

    fs::write(
        directory.path().join(".luaurc"),
        r#"{"languageMode":"strict"}"#,
    )
    .unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .current_dir(directory.path())
        .args(["analyze", "--mode", "nocheck", "-"])
        .write_stdin("local value: number = 'wrong'\nreturn value")
        .assert()
        .success();

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--mode", "invalid", "-"])
        .assert()
        .code(2);
}

#[test]
fn instar_mode_inherits_and_overrides_upstream_settings() {
    let source = "local function field(value) return value.name end\nreturn field(1)";

    for solver in ["new", "old"] {
        for upstream in [".luaurc", ".config.luau", "config.luau"] {
            let directory = tempfile::tempdir().unwrap();
            let root = directory.path();
            let child = root.join("child");
            fs::create_dir(&child).unwrap();
            fs::write(root.join("instar.toml"), "mode = 'strict'").unwrap();

            let settings = if upstream == ".luaurc" {
                r#"{"languageMode":"nocheck"}"#
            } else {
                "return {luau = {languagemode = 'nocheck'}}"
            };

            fs::write(root.join(upstream), settings).unwrap();

            Command::new(assert_cmd::cargo::cargo_bin!("instar"))
                .current_dir(&child)
                .args(["analyze", "--solver", solver, "-"])
                .write_stdin(source)
                .assert()
                .code(1);

            fs::write(child.join(upstream), settings).unwrap();
            let checked = "--!strict\nlocal value: number = true\nreturn value";
            let unchecked = "--!nocheck\nlocal value: number = true\nreturn value";

            for (configuration, mode, input, status) in [
                ("", None, source, 0),
                ("mode = 'strict'", None, source, 1),
                ("mode = 'nonstrict'", None, source, 0),
                ("mode = 'nocheck'", None, source, 0),
                ("mode = 'nocheck'", Some("strict"), source, 1),
                ("mode = 'strict'", Some("nonstrict"), source, 0),
                ("mode = 'strict'", Some("nocheck"), source, 0),
                ("mode = 'strict'", Some("strict"), unchecked, 0),
                ("mode = 'nocheck'", Some("nocheck"), checked, 1),
            ] {
                fs::write(child.join("instar.toml"), configuration).unwrap();
                let mut command = Command::new(assert_cmd::cargo::cargo_bin!("instar"));

                command
                    .current_dir(&child)
                    .args(["analyze", "--solver", solver]);

                if let Some(mode) = mode {
                    command.args(["--mode", mode]);
                }

                command.arg("-").write_stdin(input).assert().code(status);
            }

            fs::write(child.join("instar.toml"), "mode = 'invalid'").unwrap();

            Command::new(assert_cmd::cargo::cargo_bin!("instar"))
                .current_dir(&child)
                .args(["analyze", "-"])
                .write_stdin(source)
                .assert()
                .code(1);
        }
    }
}

#[test]
fn named_standard_input_resolves_imports_from_its_filename() {
    let directory = tempfile::tempdir().unwrap();

    fs::write(
        directory.path().join(".luaurc"),
        r#"{"languageMode":"strict"}"#,
    )
    .unwrap();

    fs::write(directory.path().join("value.luau"), "return 42").unwrap();
    let filename = directory.path().join("unsaved.luau");

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--filename"])
        .arg(&filename)
        .arg("-")
        .write_stdin("local value: number = require('./value')\nreturn value")
        .assert()
        .success()
        .stderr("");

    let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--filename"])
        .arg(&filename)
        .arg("-")
        .write_stdin("local value: string = require('./value')\nreturn value")
        .assert()
        .code(1);

    let stderr = String::from_utf8_lossy(&assertion.get_output().stderr);
    assert!(stderr.contains("unsaved.luau"), "{stderr}");
    assert!(stderr.contains("TypeError"), "{stderr}");
    assert!(!filename.exists());
}

#[test]
fn resolves_unicode_module_paths() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("日本語");
    fs::create_dir(&folder).unwrap();
    fs::write(folder.join("値.luau"), "return 1").unwrap();
    let main = folder.join("入口.luau");

    fs::write(
        &main,
        "--!strict\nlocal value: number = require('./値')\nreturn value",
    )
    .unwrap();

    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .arg("analyze")
        .arg(main)
        .assert()
        .success()
        .stderr("");
}

#[test]
fn annotations_preserve_source_and_produce_checkable_types() {
    let source = "-- retained\r\nlocal function pair(first, second) return first, second end\r\nlocal function finish() end\r\nfinish()\r\nlocal value = 2 -- retained too\r\nlocal text: string = '雪'\r\nreturn pair(value, text)\r\n";

    for solver in ["new", "old"] {
        let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args([
                "analyze",
                "--mode",
                "strict",
                "--solver",
                solver,
                "--annotate",
                "-",
            ])
            .write_stdin(source)
            .assert()
            .success()
            .stderr("");

        let output = &assertion.get_output().stdout;
        let text = String::from_utf8_lossy(output);
        assert!(text.starts_with("-- retained\r\n"), "{text}");

        assert!(
            text.contains("local value: number = 2 -- retained too\r\n"),
            "{text}"
        );

        assert!(text.contains("local text: string = '雪'\r\n"), "{text}");
        assert!(text.contains("finish(): ()"), "{text}");
        assert!(text.contains("pair<"), "{text}");

        Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--mode", "strict", "--solver", solver, "-"])
            .write_stdin(output.clone())
            .assert()
            .success()
            .stderr("");
    }
}

#[test]
fn annotations_handle_recursive_types_and_analysis_errors() {
    for source in [
        "type Node = {value: number, next: Node?}\nlocal function read(node: Node)\nlocal next = node.next\nreturn next\nend\nreturn read",
        "local value = missing\nreturn value",
    ] {
        let assertion = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--mode", "strict", "--annotate", "-"])
            .write_stdin(source)
            .assert();

        let output = assertion.get_output();
        assert!(matches!(output.status.code(), Some(0 | 1)));
        assert_ne!(output.stdout, [] as [u8; 0]);
        let diagnostics = String::from_utf8_lossy(&output.stderr);
        assert!(!diagnostics.contains("native analysis"), "{diagnostics}");
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(!text.contains("*error-type*"), "{text}");

        let checked = Command::new(assert_cmd::cargo::cargo_bin!("instar"))
            .args(["analyze", "--mode", "strict", "-"])
            .write_stdin(output.stdout.clone())
            .assert();

        let diagnostics = String::from_utf8_lossy(&checked.get_output().stderr);
        assert!(!diagnostics.contains("SyntaxError"), "{diagnostics}");
        assert_eq!(checked.get_output().status.code(), output.status.code());
    }
}

#[test]
fn runs_from_a_relocated_binary_and_supports_upstream_options() {
    let directory = tempfile::tempdir().unwrap();

    let executable = directory.path().join(if cfg!(windows) {
        "instar.exe"
    } else {
        "instar"
    });

    fs::copy(assert_cmd::cargo::cargo_bin!("instar"), &executable).unwrap();
    fs::write(directory.path().join("source.luau"), "return 1\n").unwrap();

    let assertion = Command::new(executable)
        .current_dir(directory.path())
        .args([
            "analyze",
            "--mode=strict",
            "--solver=new",
            "--annotate",
            "source.luau",
        ])
        .assert()
        .success()
        .stderr("");

    let stdout = String::from_utf8_lossy(&assertion.get_output().stdout);
    assert!(stdout.contains("return 1"), "{stdout}");
}
