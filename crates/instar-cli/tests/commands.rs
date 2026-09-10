use assert_cmd::Command;

#[test]
fn help_and_version_describe_the_cli() {
    let output = Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("--help")
        .assert()
        .success()
        .get_output()
        .clone();
    let help = String::from_utf8(output.stdout).expect("help is UTF-8");
    for name in ["analyze", "format", "lint", "lsp", "build"] {
        assert!(
            help.lines()
                .any(|line| line.split_whitespace().next() == Some(name)),
            "{help}"
        );
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .args([name, "--help"])
            .assert()
            .success();
    }
    Command::new(env!("CARGO_BIN_EXE_instar"))
        .arg("--version")
        .assert()
        .success()
        .stdout(concat!("instar ", env!("CARGO_PKG_VERSION"), "\n"));
    Command::new(env!("CARGO_BIN_EXE_instar"))
        .assert()
        .failure();
}

#[test]
fn placeholders_fail_without_protocol_output() {
    for args in [
        &["format"][..],
        &["lint"],
        &["lint", "--fix"],
        &["lsp"],
        &["build"],
    ] {
        Command::new(env!("CARGO_BIN_EXE_instar"))
            .args(args)
            .assert()
            .failure()
            .stdout("")
            .stderr(format!("{}: not implemented\n", args.join(" ")));
    }
}

#[test]
fn fix_is_only_a_lint_option() {
    for name in ["analyze", "format", "lsp", "build"] {
        let output = Command::new(env!("CARGO_BIN_EXE_instar"))
            .args([name, "--fix"])
            .assert()
            .failure()
            .stdout("")
            .get_output()
            .clone();
        let error = String::from_utf8(output.stderr).expect("argument errors are UTF-8");
        assert!(error.contains("unexpected argument '--fix'"), "{error}");
    }
}
