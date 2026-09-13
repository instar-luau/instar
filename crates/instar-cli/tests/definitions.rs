//! Loading and resolving external Luau definition sources.

use assert_cmd::Command;
use std::fs;

#[test]
fn definition_inputs_support_files_directories_and_stdin() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    fs::write(root.join("instar.toml"), "definitions=['globals.d.luau']").unwrap();
    fs::write(root.join("globals.d.luau"), "declare value: number").unwrap();
    fs::write(root.join("main.luau"), "--!strict\nreturn value").unwrap();

    for input in ["globals.d.luau", "."] {
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["analyze", input])
            .assert()
            .success()
            .stdout("")
            .stderr("");
    }

    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(root)
        .args(["analyze", "-", "--filename", "globals.d.luau"])
        .write_stdin("declare value: Missing")
        .assert()
        .failure()
        .get_output()
        .stderr
        .clone();

    let diagnostics = String::from_utf8(output).unwrap();
    assert!(diagnostics.contains("globals.d.luau"), "{diagnostics}");
    assert!(diagnostics.contains("TypeError"), "{diagnostics}");

    assert_eq!(
        fs::read_to_string(root.join("globals.d.luau")).unwrap(),
        "declare value: number"
    );
}
