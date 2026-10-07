//! Isolated native configuration safeguards.

use std::{
    io,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

use instar_bridge::{Configuration, Mode};

fn isolated(name: &str) -> bool {
    if std::env::var("INSTAR_EXTRACTION_CASE").as_deref() == Ok(name) {
        return false;
    }

    let mut child =
        Command::new(std::env::current_exe().expect("test executable must be available"))
            .args(["--exact", name, "--nocapture"])
            .env("INSTAR_EXTRACTION_CASE", name)
            .spawn()
            .expect("isolated extraction case must start");

    let started = Instant::now();

    loop {
        if let Some(status) = child
            .try_wait()
            .expect("isolated extraction case must be observable")
        {
            assert!(
                status.success(),
                "isolated extraction case failed: {status}"
            );

            return true;
        }

        if started.elapsed() > Duration::from_secs(10) {
            child.kill().expect("stalled extraction case must stop");

            child
                .wait()
                .expect("stopped extraction case must be reaped");

            panic!("isolated extraction case exceeded its watchdog");
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}

fn rejected(
    name: &str,
    source: &str,
    path: &Path,
    message: &str,
    kind: io::ErrorKind,
    timeout: Duration,
) {
    if isolated(name) {
        return;
    }

    let mut configuration = Configuration::new().expect("native defaults must load");

    configuration
        .apply(
            include_str!("fixtures/strict/.config.luau"),
            Path::new(".config.luau"),
            Duration::from_secs(2),
        )
        .expect("valid layer must load");

    let previous = configuration.snapshot().expect("snapshot must load");
    assert_eq!(previous.mode, Mode::Strict);

    let error = configuration
        .apply(source, path, timeout)
        .expect_err("invalid layer must fail");

    assert_eq!(error.kind(), kind);
    assert!(error.to_string().contains(&path.display().to_string()));
    assert!(error.to_string().contains(message));

    assert_eq!(
        configuration.snapshot().expect("snapshot must load"),
        previous
    );
}

#[test]
fn cyclic_table_is_rejected() {
    rejected(
        "cyclic_table_is_rejected",
        include_str!("fixtures/cycle/.config.luau"),
        Path::new(".config.luau"),
        "cycle",
        io::ErrorKind::InvalidData,
        Duration::from_secs(2),
    );
}

#[test]
fn deep_table_is_rejected() {
    rejected(
        "deep_table_is_rejected",
        include_str!("fixtures/depth/.config.luau"),
        Path::new(".config.luau"),
        "depth limit",
        io::ErrorKind::InvalidData,
        Duration::from_secs(2),
    );
}

#[test]
fn timeout_cannot_be_caught_by_configuration() {
    rejected(
        "timeout_cannot_be_caught_by_configuration",
        include_str!("fixtures/timeout/.config.luau"),
        Path::new(".config.luau"),
        "timed out",
        io::ErrorKind::TimedOut,
        Duration::from_millis(50),
    );
}

#[test]
fn timeout_escapes_unyieldable_callbacks() {
    rejected(
        "timeout_escapes_unyieldable_callbacks",
        include_str!("fixtures/callback/.config.luau"),
        Path::new(".config.luau"),
        "timed out",
        io::ErrorKind::TimedOut,
        Duration::from_millis(50),
    );
}

#[test]
fn require_is_unavailable() {
    rejected(
        "require_is_unavailable",
        include_str!("fixtures/require/.config.luau"),
        Path::new(".config.luau"),
        "nil value",
        io::ErrorKind::InvalidData,
        Duration::from_secs(2),
    );
}

#[test]
fn native_parse_failure_is_transactional() {
    rejected(
        "native_parse_failure_is_transactional",
        include_str!("fixtures/parsing/.luaurc"),
        Path::new(".luaurc"),
        "Unknown key unknown",
        io::ErrorKind::InvalidData,
        Duration::from_secs(2),
    );
}

#[test]
fn sandbox_has_no_external_access() {
    if isolated("sandbox_has_no_external_access") {
        return;
    }

    let mut configuration = Configuration::new().expect("native defaults must load");

    configuration
        .apply(
            include_str!("fixtures/sandbox/.config.luau"),
            Path::new(".config.luau"),
            Duration::from_secs(2),
        )
        .expect("sandbox checks must pass");

    assert_eq!(
        configuration.snapshot().expect("snapshot must load").mode,
        Mode::Strict
    );
}

#[test]
fn upstream_defaults_are_preserved() {
    let snapshot = Configuration::new()
        .expect("native defaults must load")
        .snapshot()
        .expect("snapshot must load");

    assert_eq!(snapshot.mode, Mode::NonStrict);
    assert!(!snapshot.lint_errors);
    assert!(snapshot.type_errors);
    assert_eq!(snapshot.globals, Vec::<String>::new());
    assert!(snapshot.aliases.is_empty());
    assert!(!snapshot.lint.is_empty());

    assert!(
        snapshot
            .lint
            .values()
            .all(|policy| policy.enabled && !policy.fatal)
    );
}
