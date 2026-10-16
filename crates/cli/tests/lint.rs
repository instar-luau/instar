//! Regression coverage for syntax and native lint findings.

use std::{
    io::Write as _,
    process::{Command, Stdio},
};

#[test]
fn ignored_escape_is_reported() {
    finding("bad_string_escape", "return \"\\q\"");
}

#[test]
fn established_signature_checks_argument_count() {
    finding(
        "mismatched_argument_count",
        "local function identity(value: number) return value end\nreturn identity(1, 2)",
    );
}

#[test]
fn builtin_signature_checks_argument_count() {
    finding("mismatched_argument_count", "return math.abs()");
}

#[test]
fn byte_order_mark_preserves_native_findings() {
    finding(
        "self_assignment",
        "\u{feff}local value=1\r\nvalue=value\r\nreturn value\r\n",
    );
}

#[test]
fn selected_upstream_warning_runs_in_checking_modes() {
    finding("UnknownGlobal", "return missing");
    finding("UnknownGlobal", "--!strict\nreturn missing");
}

fn finding(rule: &str, source: &str) {
    let directory = tempfile::tempdir().expect("temporary project");

    let mut child = Command::new(env!("CARGO_BIN_EXE_instar"))
        .current_dir(directory.path())
        .args([
            "lint", "--quiet", "--color", "never", "--rule", rule, "--", "-",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start lint");

    child
        .stdin
        .take()
        .expect("stdin pipe")
        .write_all(source.as_bytes())
        .expect("write source");

    let output = child.wait_with_output().expect("finish lint");
    let diagnostics = String::from_utf8(output.stderr).expect("UTF-8 diagnostics");
    assert!(output.status.success(), "{diagnostics}");
    assert_eq!(output.stdout.as_slice(), b"");

    assert!(
        diagnostics.contains(&format!("warning[{rule}]")),
        "{diagnostics}"
    );
}
