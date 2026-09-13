//! Graft execution, protocol validation, and transformation behavior.

use assert_cmd::Command;
use std::fs;

#[path = "../../instar-core/tests/support/graft.rs"]
mod support;

#[path = "../../instar-core/tests/support/native.rs"]
mod native;

#[test]
fn graft_layouts_reach_standard_output_checks_and_file_writes() {
    let reply = r#"{"version":1,"document":{"sequence":[{"text":"local value ="},{"indent":{"sequence":["hard",{"host":{"start":14,"end":15,"parse":"expression"}}]}}]}}"#;

    for directory in [
        support::fixture(reply, "instar_format"),
        native::fixture(reply),
        support::luau(
            "return table.freeze({format=function() return {version=1, document={sequence={{text='local value ='},{indent={sequence={'hard',{host={start=14,['end']=15,parse='expression'}}}}}}}} end})",
            "format",
        ),
    ] {
        let root = directory.path();

        fs::write(
            root.join("instar.toml"),
            fs::read_to_string(root.join("instar.toml")).unwrap()
                + "\n[grafts]\nexample = { path = '.' }",
        )
        .unwrap();

        fs::write(root.join("source.luau"), "local value=1").unwrap();

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["graft", "install"])
            .assert()
            .success();

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["format", "-"])
            .write_stdin("local value=1")
            .assert()
            .success()
            .stdout("local value =\n\t1\n");

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["format", "--check", "source.luau"])
            .assert()
            .failure()
            .stdout("");

        assert_eq!(
            fs::read(root.join("source.luau")).unwrap(),
            b"local value=1"
        );

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["format", "source.luau"])
            .assert()
            .success()
            .stdout("");

        assert_eq!(
            fs::read(root.join("source.luau")).unwrap(),
            b"local value =\n\t1\n"
        );

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["format", "--check", "source.luau"])
            .assert()
            .success()
            .stdout("");
    }
}

#[test]
fn invalid_graft_output_leaves_files_untouched() {
    let reply = r#"{"version":1,"document":{"text":"return 2"}}"#;

    for directory in [
        support::fixture(reply, "instar_format"),
        native::fixture(reply),
        support::luau(
            "return table.freeze({format=function() return {version=1,document={text='return 2'}} end})",
            "format",
        ),
    ] {
        let root = directory.path();

        fs::write(
            root.join("instar.toml"),
            fs::read_to_string(root.join("instar.toml")).unwrap()
                + "\n[grafts]\nexample = { path = '.' }",
        )
        .unwrap();

        fs::write(root.join("source.luau"), "return 1").unwrap();

        Command::new(env!("CARGO_BIN_EXE_instar"))
            .current_dir(root)
            .args(["format", "source.luau"])
            .assert()
            .failure()
            .stdout("");

        assert_eq!(fs::read(root.join("source.luau")).unwrap(), b"return 1");
    }
}
