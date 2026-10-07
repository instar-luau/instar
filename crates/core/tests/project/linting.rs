//! Native and Instar lint policies over shared contextual project sources.
use std::{io, time::Duration};

use instar_analysis::{self as analysis, Completion, Options, Reason};

use instar_core::{
    analysis::{Entry, Origin},
    project::Change,
    roblox,
};

use instar_lint::{self as lint, Diagnostic, Kind, Level, Rule};

use crate::support::{Directory, options, project};

fn rule(result: &lint::Result<Origin>, name: Rule) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == Kind::Rule(name))
}

#[test]
fn native_and_instar_policies_are_independent_and_checker_stays_clean() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/native/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/native/entry.luau"),
    )?;

    let mut project = project();
    let entries = [Entry::new(entry.clone())];
    let before = project.check(&entries, &options())?;
    assert_eq!(before.diagnostics, Vec::new());
    let linted = project.lint(&entries, &options())?;
    assert_eq!(linted.completion, Completion::Complete);

    assert!(linted.diagnostics.iter().any(
        |diagnostic| matches!(&diagnostic.kind, Kind::Native { name, .. } if name == "LocalUnused")
            && diagnostic.level == Level::Warn
    ));

    assert!(linted.diagnostics.iter().any(|diagnostic| diagnostic.kind
        == Kind::Rule(Rule::UnusedVariable)
        && diagnostic.level == Level::Info));

    assert_eq!(project.check(&entries, &options())?, before);

    project.change(Change::Overlay {
        path: directory.path().join(".luaurc"),
        text: Some(include_str!("../fixtures/linting/native/fatal.json").to_owned()),
    })?;

    let fatal = project.lint(&entries, &options())?;

    assert!(
        fatal
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Native { .. })
                && diagnostic.level == Level::Deny)
    );

    project.change(Change::Overlay {
        path: entry,
        text: Some(include_str!("../fixtures/linting/native/input.luau").to_owned()),
    })?;

    let directive = project.lint(&entries, &options())?;

    assert!(
        !directive
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Native { .. }))
    );

    assert!(rule(&directive, Rule::UnusedVariable));

    Ok(())
}

#[test]
fn selection_chooses_entries_and_reports_reachable_dependencies() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/selection/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/selection/entry.luau"),
    )?;

    let dependency = directory.file(
        "dependency.luau",
        include_str!("../fixtures/linting/selection/dependency.luau"),
    )?;

    let mut project = project();
    let entries = [Entry::new(entry)];
    let result = project.lint(&entries, &options())?;
    assert_eq!(result.modules.len(), 2);
    assert_eq!(result.modules, project.check(&entries, &options())?.modules);

    assert!(
        result
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == dependency
                    && diagnostic.kind == Kind::Rule(Rule::UnusedVariable)
            )
    );

    assert!(result.diagnostics.iter().any(|diagnostic| diagnostic.location.module.source() == dependency && matches!(&diagnostic.kind, Kind::Native { name, .. } if name == "LocalUnused")));

    assert!(
        result
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == dependency
                    && matches!(
                        diagnostic.kind,
                        Kind::Analysis(analysis::Kind::Resolution { .. })
                    )
            )
    );

    assert_eq!(
        project
            .lint(&[Entry::new(dependency)], &options())?
            .diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn shared_typed_require_aliases_drive_restrictions() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        ".luaurc",
        include_str!("../fixtures/linting/shared/.luaurc"),
    )?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/shared/instar.toml"),
    )?;

    directory.file("dependency.luau", "return 1")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/shared/entry.luau"),
    )?;

    let mut project = project();
    let result = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert!(rule(&result, Rule::RestrictedModulePaths));
    assert!(!rule(&result, Rule::NonConstRequire));

    assert!(result.diagnostics.iter().any(|diagnostic| diagnostic.kind
        == Kind::Rule(Rule::RestrictedModulePaths)
        && &project.source(&entry).expect("source").text
            [diagnostic.location.range[0]..diagnostic.location.range[1]]
            == "path"));

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(include_str!("../fixtures/linting/shared/input.luau").to_owned()),
    })?;

    let shadowed = project.lint(&[Entry::new(entry)], &options())?;
    assert!(!rule(&shadowed, Rule::RestrictedModulePaths));
    assert!(!rule(&shadowed, Rule::NonConstRequire));

    Ok(())
}

#[test]
fn syntax_only_rules_do_not_load_declaration_environments() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        ".luaurc",
        include_str!("../fixtures/linting/shared/.luaurc"),
    )?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/syntax/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/syntax/entry.luau"),
    )?;

    let result = project().lint(&[Entry::new(entry)], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert!(rule(&result, Rule::DivideByZero));

    assert!(!result.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.kind,
        Kind::Analysis(analysis::Kind::Analysis { .. })
    )));

    Ok(())
}

#[test]
fn inferred_any_uses_native_types_and_per_module_selection() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/inferred/instar.toml"),
    )?;

    directory.file(
        "dependency/instar.toml",
        include_str!("../fixtures/linting/inferred/dependency/instar.toml"),
    )?;

    directory.file(
        "dependency/.luaurc",
        include_str!("../fixtures/linting/inferred/dependency/.luaurc"),
    )?;

    directory.file("dependency/init.luau", "return 1")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/inferred/entry.luau"),
    )?;

    let mut project = project();
    let strict = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(strict.completion, Completion::Complete);
    assert!(!rule(&strict, Rule::ImplicitAnyLocal));
    assert!(!rule(&strict, Rule::ImplicitAnyParameter));

    project.change(Change::Overlay {
        path: directory.path().join(".luaurc"),
        text: Some(include_str!("../fixtures/linting/inferred/nonstrict.json").to_owned()),
    })?;

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(include_str!("../fixtures/linting/inferred/input.luau").to_owned()),
    })?;

    let inferred = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(inferred.completion, Completion::Complete);
    assert!(!rule(&inferred, Rule::ImplicitAnyParameter));

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(include_str!("../fixtures/linting/inferred/replacement.luau").to_owned()),
    })?;

    let nonstrict = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(nonstrict.completion, Completion::Complete);
    assert!(rule(&nonstrict, Rule::ImplicitAnyParameter));

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(include_str!("../fixtures/linting/inferred/invalid.luau").to_owned()),
    })?;

    let nocheck = project.lint(&[Entry::new(entry)], &options())?;
    assert_eq!(nocheck.completion, Completion::Incomplete(Reason::Analysis));

    assert!(nocheck.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.kind,
        Kind::Analysis(analysis::Kind::Analysis { .. })
    )));

    Ok(())
}

#[test]
fn missing_and_invalid_definitions_remain_explicit_with_syntax_findings() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/missing/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/missing/entry.luau"),
    )?;

    let mut project = project();
    let missing = project.lint(&[Entry::new(entry.clone())], &options())?;

    assert_eq!(
        missing.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(rule(&missing, Rule::DivideByZero));

    assert!(missing.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.location.module,
        Origin::Definition(_)
    ) && matches!(
        diagnostic.kind,
        Kind::Analysis(analysis::Kind::Analysis { .. })
    )));

    let definition = directory.path().join("globals.luau");

    project.change(Change::Overlay {
        path: definition.clone(),
        text: Some(include_str!("../fixtures/linting/missing/invalid.luau").to_owned()),
    })?;

    let invalid = project.lint(&[Entry::new(entry.clone())], &options())?;

    assert_eq!(
        invalid.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(
        invalid
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == definition
                    && matches!(
                        diagnostic.kind,
                        Kind::Analysis(analysis::Kind::Syntax { .. })
                    )
            )
    );

    project.change(Change::Overlay {
        path: definition,
        text: Some(include_str!("../fixtures/linting/missing/definition.luau").to_owned()),
    })?;

    assert_eq!(
        project.lint(&[Entry::new(entry)], &options())?.completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn invalid_roblox_metadata_keeps_known_lint_findings() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/invalid/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/invalid/entry.luau"),
    )?;

    let mut project = project();

    project.change(Change::Overlay {
        path: roblox::cache_directory()?.join("metadata.json"),
        text: Some("{}".to_owned()),
    })?;

    let result = project.lint(&[Entry::new(entry)], &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(rule(&result, Rule::RobloxIncorrectColor3NewBounds));

    assert!(result.diagnostics.iter().any(
        |diagnostic| diagnostic.kind == Kind::Analysis(analysis::Kind::Analysis { code: None })
    ));

    Ok(())
}

#[test]
fn source_updates_and_contexts_keep_exact_revision_identity() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/source/instar.toml"),
    )?;

    directory.file(
        "globals.luau",
        include_str!("../fixtures/linting/source/globals.luau"),
    )?;

    let shared = directory.file(
        "shared.luau",
        include_str!("../fixtures/linting/syntax/entry.luau"),
    )?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/linting/source/sourcemap.json"),
    )?;

    let mut project = project();
    let contexts = project.contexts(&shared)?;
    assert_eq!(contexts.len(), 2);

    let entries = contexts
        .iter()
        .map(|module| Entry {
            source: shared.clone(),
            context: Some(module.identity.clone()),
        })
        .collect::<Vec<_>>();

    let result = project.lint(&entries, &options())?;
    assert_eq!(result.modules.len(), 2);

    let findings = result
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.kind == Kind::Rule(Rule::DivideByZero))
        .collect::<Vec<_>>();

    assert_eq!(findings.len(), 2);
    assert_ne!(findings[0].location.module, findings[1].location.module);
    let revision = findings[0].location.revision;

    project.change(Change::Overlay {
        path: shared,
        text: Some(include_str!("../fixtures/linting/source/replacement.luau").to_owned()),
    })?;

    let updated = project.lint(&entries, &options())?;
    assert!(!rule(&updated, Rule::DivideByZero));
    assert_ne!(project.source(&entries[0].source)?.revision, revision);

    Ok(())
}

#[test]
fn cancellation_and_timeout_are_explicit() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    let entry = directory.file("entry.luau", "return 1")?;
    let mut project = project();
    let entries = [Entry::new(entry)];

    assert_eq!(
        project
            .lint(&entries, &Options::new(Duration::ZERO))?
            .completion,
        Completion::Incomplete(Reason::Timeout)
    );

    let cancelled = options();
    cancelled.cancellation.cancel();

    assert_eq!(
        project.lint(&entries, &cancelled)?.completion,
        Completion::Incomplete(Reason::Cancelled)
    );

    Ok(())
}

#[test]
fn native_analysis_identity_survives_lint_conversion() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/linting/conversion/strict.json"
    )))?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/conversion/entry.luau"),
    )?;

    let mut project = project();
    let entries = [Entry::new(entry.clone())];

    for source in [
        include_str!("../fixtures/linting/conversion/entry.luau"),
        include_str!("../fixtures/linting/conversion/input.luau"),
        "local =",
    ] {
        project.change(Change::Overlay {
            path: entry.clone(),
            text: Some(source.to_owned()),
        })?;

        let checked = project.check(&entries, &options())?;
        let linted = project.lint(&entries, &options())?;
        assert_eq!(linted.completion, checked.completion);
        assert_eq!(linted.modules, checked.modules);
        assert_ne!(checked.diagnostics, Vec::new());

        assert!(
            checked
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.kind.native_code().is_some())
        );

        assert_eq!(
            linted.diagnostics,
            checked
                .diagnostics
                .into_iter()
                .map(Diagnostic::from)
                .collect::<Vec<_>>()
        );
    }

    project.change(Change::Overlay {
        path: entry,
        text: Some("return 1".to_owned()),
    })?;

    project.change(Change::Overlay {
        path: directory.path().join("instar.toml"),
        text: Some(include_str!("../fixtures/linting/missing/instar.toml").to_owned()),
    })?;

    for declaration in [
        include_str!("../fixtures/linting/missing/invalid.luau"),
        include_str!("../fixtures/linting/conversion/definition.luau"),
    ] {
        project.change(Change::Overlay {
            path: directory.path().join("globals.luau"),
            text: Some(declaration.to_owned()),
        })?;

        let checked = project.check(&entries, &options())?;
        let linted = project.lint(&entries, &options())?;

        assert_eq!(
            checked.completion,
            Completion::Incomplete(Reason::Environment)
        );

        assert_eq!(linted.completion, checked.completion);
        assert_ne!(checked.diagnostics, Vec::new());

        assert!(
            checked
                .diagnostics
                .iter()
                .all(|diagnostic| diagnostic.kind.native_code().is_some())
        );

        assert_eq!(
            linted.diagnostics,
            checked
                .diagnostics
                .into_iter()
                .map(Diagnostic::from)
                .collect::<Vec<_>>()
        );
    }

    Ok(())
}
