//! Command output modes.

use std::{
    error::Error,
    fs,
    io::Write as _,
    process::{Command, Output, Stdio},
};

const EXECUTABLE: &str = env!("CARGO_BIN_EXE_instar");

fn format(flags: &[&str], source: &str) -> Result<Output, Box<dyn Error>> {
    let directory = tempfile::tempdir()?;

    let mut child = Command::new(EXECUTABLE)
        .args(flags)
        .args(["--color", "never", "format", "-"])
        .arg("--root")
        .arg(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    child
        .stdin
        .take()
        .expect("formatter stdin")
        .write_all(source.as_bytes())?;

    Ok(child.wait_with_output()?)
}

#[test]
fn modes() -> Result<(), Box<dyn Error>> {
    for (flags, is_detailed, is_debug, is_quiet) in [
        (Vec::new(), false, false, false),
        (vec!["--verbose"], true, false, false),
        (vec!["--debug"], true, true, false),
        (vec!["--verbose", "--debug"], true, true, false),
        (vec!["--quiet"], false, false, true),
    ] {
        let output = format(&flags, "return 1\n")?;
        let stderr = String::from_utf8(output.stderr)?;
        assert!(output.status.success(), "{flags:?}: {stderr}");
        assert_eq!(String::from_utf8(output.stdout)?, "return 1\n");

        assert_eq!(
            stderr.contains("Project  "),
            is_detailed,
            "{flags:?}: {stderr}"
        );

        assert_eq!(
            stderr.contains("Workers  "),
            is_detailed,
            "{flags:?}: {stderr}"
        );

        assert_eq!(
            stderr.contains("Formatting  stdin.luau"),
            is_detailed,
            "{flags:?}: {stderr}"
        );

        assert_eq!(
            stderr.contains("Processing batch"),
            is_debug,
            "{flags:?}: {stderr}"
        );

        assert_eq!(
            stderr.contains("Formatted 1 file in "),
            !is_quiet,
            "{flags:?}: {stderr}"
        );

        if is_quiet {
            assert!(stderr.is_empty(), "{stderr}");
        }
    }

    Ok(())
}

#[test]
fn conflicts() -> Result<(), Box<dyn Error>> {
    for flags in [
        ["--quiet", "--verbose"],
        ["--verbose", "--quiet"],
        ["--quiet", "--debug"],
        ["--debug", "--quiet"],
    ] {
        let output = Command::new(EXECUTABLE)
            .args(flags)
            .args(["--color", "never", "format"])
            .output()?;

        let stderr = String::from_utf8(output.stderr)?;
        assert_eq!(output.status.code(), Some(2), "{flags:?}: {stderr}");

        assert!(
            stderr.contains("cannot be used with"),
            "{flags:?}: {stderr}"
        );

        assert_eq!(output.stdout, b"");
    }

    Ok(())
}

#[test]
fn diagnostics() -> Result<(), Box<dyn Error>> {
    let output = format(&["--quiet"], "local =\n")?;
    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.lines().any(|line| line == "format"), "{stderr}");
    assert!(stderr.contains("error:"), "{stderr}");
    assert!(stderr.contains("stdin.luau:1:"), "{stderr}");
    assert!(!stderr.contains("Formatted"), "{stderr}");
    assert_eq!(output.stdout, b"");

    Ok(())
}

#[test]
fn required() -> Result<(), Box<dyn Error>> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("source.luau");
    fs::write(&path, "local value=1\nreturn value\n")?;

    let output = Command::new(EXECUTABLE)
        .args(["--quiet", "--color", "never", "format", "--check"])
        .arg("--root")
        .arg(directory.path())
        .output()?;

    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(output.status.code(), Some(1), "{stderr}");
    assert!(stderr.contains("Needs formatting  source.luau"), "{stderr}");
    assert!(!stderr.contains("Checked formatting of"), "{stderr}");
    assert_eq!(output.stdout, b"");
    assert_eq!(fs::read_to_string(path)?, "local value=1\nreturn value\n");

    let output = Command::new(EXECUTABLE)
        .args(["--quiet", "--color", "never", "format"])
        .arg("--root")
        .arg(directory.path().join("missing"))
        .output()?;

    let stderr = String::from_utf8(output.stderr)?;
    assert_eq!(output.status.code(), Some(2), "{stderr}");

    assert!(
        stderr.contains("error: Could not resolve project root"),
        "{stderr}"
    );

    Ok(())
}
