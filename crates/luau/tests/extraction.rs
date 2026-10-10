//! Isolated native configuration safeguards.

use std::{
    io,
    time::{Duration, Instant},
};

use instar_luau::{ConfigurationEvaluator, ConfigurationKey, ConfigurationValue};
use instar_shared::{Completion, DiagnosticCategory, Reason};

use crate::support::{configuration, options};

fn rejected(
    source: &str,
    executable: bool,
    message: &str,
    completion: &Completion,
    duration: Duration,
) -> io::Result<()> {
    let mut evaluator = ConfigurationEvaluator::default();
    let valid = configuration(include_str!("fixtures/strict/instar.luau"), true)?;
    let previous = evaluator.evaluate(&valid, &options(Duration::from_secs(2)))?;
    assert_eq!(previous.completion, Completion::Complete);
    let checking = previous.values.as_ref().expect("valid layer values");

    assert!(
        matches!(checking, ConfigurationValue::Table(entries) if entries.iter().any(|entry| entry.key == ConfigurationKey::String("checking".to_owned())))
    );

    let invalid = configuration(source, executable)?;
    let started = Instant::now();
    let failed = evaluator.evaluate(&invalid, &options(duration))?;
    assert_eq!(&failed.completion, completion);
    assert!(failed.values.is_none());
    assert_eq!(failed.source, invalid.source.version);

    if !message.is_empty() {
        assert!(
            failed
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains(message))
        );

        assert!(
            failed
                .diagnostics
                .iter()
                .all(
                    |diagnostic| diagnostic.primary.source == invalid.source.version
                        && diagnostic.category == DiagnosticCategory::Configuration
                )
        );
    }

    if duration <= Duration::from_millis(50) {
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    assert_eq!(
        evaluator.evaluate(&valid, &options(Duration::from_secs(2)))?,
        previous
    );

    evaluator.finish()
}

#[test]
fn cyclic_table_is_rejected() -> io::Result<()> {
    rejected(
        include_str!("fixtures/cycle/instar.luau"),
        true,
        "cycle",
        &Completion::Incomplete(vec![Reason::UnavailableSemantics]),
        Duration::from_secs(2),
    )
}

#[test]
fn deep_table_is_rejected() -> io::Result<()> {
    rejected(
        include_str!("fixtures/depth/instar.luau"),
        true,
        "depth limit",
        &Completion::Incomplete(vec![Reason::AnalysisLimit]),
        Duration::from_secs(2),
    )
}

#[test]
fn timeout_cannot_be_caught_by_configuration() -> io::Result<()> {
    rejected(
        include_str!("fixtures/timeout/instar.luau"),
        true,
        "",
        &Completion::TimedOut,
        Duration::from_millis(50),
    )
}

#[test]
fn timeout_escapes_unyieldable_callbacks() -> io::Result<()> {
    rejected(
        include_str!("fixtures/callback/instar.luau"),
        true,
        "",
        &Completion::TimedOut,
        Duration::from_millis(50),
    )
}

#[test]
fn require_is_unavailable() -> io::Result<()> {
    rejected(
        include_str!("fixtures/require/instar.luau"),
        true,
        "nil value",
        &Completion::Incomplete(vec![Reason::UnavailableSemantics]),
        Duration::from_secs(2),
    )
}

#[test]
fn native_parse_failure_is_transactional() -> io::Result<()> {
    rejected(
        include_str!("fixtures/parsing/.luaurc"),
        false,
        "Unknown key unknown",
        &Completion::Incomplete(vec![Reason::UnavailableSemantics]),
        Duration::from_secs(2),
    )
}

#[test]
fn sandbox_has_no_external_access() -> io::Result<()> {
    let mut evaluator = ConfigurationEvaluator::default();
    let input = configuration(include_str!("fixtures/sandbox/instar.luau"), true)?;
    let evaluated = evaluator.evaluate(&input, &options(Duration::from_secs(2)))?;
    assert_eq!(evaluated.completion, Completion::Complete);

    let Some(ConfigurationValue::Table(entries)) = evaluated.values else {
        panic!("root table");
    };

    let Some(ConfigurationValue::Table(checking)) = entries
        .iter()
        .find(|entry| entry.key == ConfigurationKey::String("checking".to_owned()))
        .map(|entry| &entry.value)
    else {
        panic!("checking table");
    };

    assert!(checking.iter().any(
        |entry| entry.key == ConfigurationKey::String("mode".to_owned())
            && entry.value == ConfigurationValue::String("strict".to_owned())
    ));

    evaluator.finish()
}

#[test]
fn declarative_values_preserve_generic_configuration() -> io::Result<()> {
    let mut evaluator = ConfigurationEvaluator::default();

    let input = configuration(
        "{\"languageMode\":\"nonstrict\",\"lintErrors\":false,\"typeErrors\":true,\"globals\":[],\"aliases\":{}}",
        false,
    )?;

    let evaluated = evaluator.evaluate(&input, &options(Duration::from_secs(2)))?;
    assert_eq!(evaluated.completion, Completion::Complete);

    let Some(ConfigurationValue::Table(entries)) = evaluated.values else {
        panic!("root table");
    };

    assert_eq!(entries.len(), 5);

    for (key, value) in [
        (
            "languageMode",
            ConfigurationValue::String("nonstrict".to_owned()),
        ),
        ("lintErrors", ConfigurationValue::Boolean(false)),
        ("typeErrors", ConfigurationValue::Boolean(true)),
        ("globals", ConfigurationValue::Table(Vec::new())),
        ("aliases", ConfigurationValue::Table(Vec::new())),
    ] {
        assert!(entries.iter().any(
            |entry| entry.key == ConfigurationKey::String(key.to_owned()) && entry.value == value
        ));
    }

    for (source, names, fatal) in [
        (
            include_str!("fixtures/warnings/warning/.luaurc"),
            vec!["LocalUnused"],
            true,
        ),
        (
            include_str!("fixtures/warnings/typed/.luaurc"),
            vec!["TableOperations"],
            false,
        ),
        (
            include_str!("fixtures/warnings/host/.luaurc"),
            vec!["LocalUnused"],
            false,
        ),
        (
            include_str!("fixtures/warnings/invalid/.luaurc"),
            vec!["LocalUnused", "TableLiteral"],
            false,
        ),
    ] {
        let evaluated = evaluator.evaluate(
            &configuration(source, false)?,
            &options(Duration::from_secs(2)),
        )?;

        assert_eq!(evaluated.completion, Completion::Complete);

        let Some(ConfigurationValue::Table(entries)) = evaluated.values else {
            panic!("policy root table");
        };

        let Some(ConfigurationValue::Table(lint)) = entries
            .iter()
            .find(|entry| entry.key == ConfigurationKey::String("lint".to_owned()))
            .map(|entry| &entry.value)
        else {
            panic!("upstream lint masks");
        };

        assert_eq!(lint.len(), names.len() + 1);

        assert!(lint.iter().any(
            |entry| entry.key == ConfigurationKey::String("*".to_owned())
                && entry.value == ConfigurationValue::Boolean(false)
        ));

        for name in names {
            assert!(lint.iter().any(|entry| entry.key
                == ConfigurationKey::String(name.to_owned())
                && entry.value == ConfigurationValue::Boolean(true)));
        }

        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.key == ConfigurationKey::String("lintErrors".to_owned()))
                .map(|entry| &entry.value),
            fatal.then_some(&ConfigurationValue::Boolean(true))
        );
    }

    evaluator.finish()
}
