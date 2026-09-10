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
fn explicit_strict_mode_reports_type_errors() {
    Command::new(assert_cmd::cargo::cargo_bin!("instar"))
        .args(["analyze", "--mode=strict", "-"])
        .write_stdin("local value: number = 'wrong'\nreturn value\n")
        .assert()
        .code(1);
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
