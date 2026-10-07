//! Synthetic native checking across project snapshots.
use std::{fs, io, time::Duration};

use instar_analysis::{self as analysis, Completion, Kind, Options, Reason};

use instar_core::{
    analysis::{Entry, Origin},
    project::Change,
    roblox,
};

use instar_lint::Diagnostic;

use crate::support::{Directory, options, project};

#[test]
fn imported_types_and_source_updates() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let dependency = directory.file(
        "dependency.luau",
        include_str!("../fixtures/checking/imported/dependency.luau"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/imported/entry.luau"),
    )?;

    let mut project = project();
    let entries = [Entry::new(entry.clone())];
    let first = project.check(&entries, &options())?;
    assert_eq!(first.completion, Completion::Complete);

    assert!(
        first
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Type { .. }))
    );

    assert!(
        first
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.location.range[1]
                <= project.source(&entry).expect("entry source").text.len())
    );

    project.change(Change::Overlay {
        path: dependency,
        text: Some(include_str!("../fixtures/checking/imported/replacement.luau").to_owned()),
    })?;

    let updated = project.check(&entries, &options())?;
    assert_eq!(updated.completion, Completion::Complete);
    assert!(updated.diagnostics.is_empty(), "{:?}", updated.diagnostics);

    Ok(())
}

#[test]
fn dependency_errors_are_not_filtered_or_repeated() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/dependency/instar.toml"),
    )?;

    let dependency = directory.file(
        "dependency.luau",
        include_str!("../fixtures/checking/dependency/dependency.luau"),
    )?;

    let first = directory.file(
        "first.luau",
        include_str!("../fixtures/checking/dependency/first.luau"),
    )?;

    let second = directory.file(
        "second.luau",
        include_str!("../fixtures/checking/dependency/first.luau"),
    )?;

    let mut project = project();
    let single = project.check(&[Entry::new(first.clone())], &options())?;
    let multiple = project.check(&[Entry::new(first), Entry::new(second)], &options())?;

    let dependency_errors = |result: &analysis::Result<Origin>| {
        result
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.location.module.source() == dependency.as_path())
            .count()
    };

    assert!(dependency_errors(&single) > 0);
    assert_eq!(dependency_errors(&multiple), dependency_errors(&single));

    assert_eq!(
        project
            .check(&[Entry::new(dependency)], &options())?
            .diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn cycles_are_finite_and_keep_native_identity() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let first = directory.file(
        "first.luau",
        include_str!("../fixtures/checking/cycles/first.luau"),
    )?;

    directory.file(
        "second.luau",
        include_str!("../fixtures/checking/cycles/second.luau"),
    )?;

    let result = project().check(&[Entry::new(first)], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.modules.len(), 2);
    assert_ne!(result.diagnostics, Vec::new());

    assert!(
        result
            .diagnostics
            .iter()
            .all(|diagnostic| diagnostic.kind.native_code().is_some())
    );

    Ok(())
}

#[test]
fn declarations_are_scoped_and_invalidated() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/declarations/instar.toml"),
    )?;

    let definition = directory.file(
        "globals.luau",
        include_str!("../fixtures/checking/declarations/globals.luau"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/declarations/entry.luau"),
    )?;

    directory.file(
        "isolated/instar.toml",
        include_str!("../fixtures/checking/declarations/isolated/instar.toml"),
    )?;

    let isolated = directory.file(
        "isolated/entry.luau",
        include_str!("../fixtures/checking/declarations/isolated/entry.luau"),
    )?;

    let mut project = project();

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    assert_ne!(
        project
            .check(&[Entry::new(isolated)], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: definition.clone(),
        text: Some(include_str!("../fixtures/checking/declarations/definition.luau").to_owned()),
    })?;

    assert_ne!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: definition.clone(),
        text: Some(include_str!("../fixtures/checking/declarations/invalid.luau").to_owned()),
    })?;

    let invalid = project.check(&[Entry::new(entry.clone())], &options())?;

    assert!(
        invalid
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == definition
                    && matches!(diagnostic.kind, Kind::Syntax { .. })
            )
    );

    project.change(Change::Overlay {
        path: definition,
        text: Some(include_str!("../fixtures/checking/declarations/globals.luau").to_owned()),
    })?;

    assert_eq!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn native_modes_and_lint_ownership() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let configuration = directory.path.join(".luaurc");

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/native/entry.luau"),
    )?;

    let mut project = project();

    assert_ne!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: configuration.clone(),
        text: Some(include_str!("../fixtures/checking/native/nonstrict.json").to_owned()),
    })?;

    let nonstrict = project.check(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(nonstrict.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration,
        text: Some(include_str!("../fixtures/checking/native/nocheck.json").to_owned()),
    })?;

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some("local =".to_owned()),
    })?;

    assert!(
        project
            .check(&[Entry::new(entry)], &options())?
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Syntax { .. }))
    );

    Ok(())
}

#[test]
fn unresolved_import_has_one_source_diagnostic() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/unresolved/entry.luau"),
    )?;

    let result = project().check(&[Entry::new(entry)], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.diagnostics.len(), 1);

    assert!(matches!(
        result.diagnostics[0].kind,
        Kind::Resolution { .. }
    ));

    assert!(result.diagnostics[0].location.range[0] > 0);

    Ok(())
}

#[test]
fn interruption_is_not_success_and_can_be_retried() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let entry = directory.file("entry.luau", "return 1")?;
    let mut project = project();
    let cancelled = options();
    cancelled.cancellation.cancel();

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &cancelled)?
            .completion,
        Completion::Incomplete(Reason::Cancelled)
    );

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &Options::new(Duration::ZERO))?
            .completion,
        Completion::Incomplete(Reason::Timeout)
    );

    assert_eq!(
        project.check(&[Entry::new(entry)], &options())?.completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn incomplete_environments_keep_known_source_diagnostics() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/incomplete/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/incomplete/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/incomplete/entry.luau"),
    )?;

    let mut project = project();
    let entries = [Entry::new(entry.clone())];
    let result = project.check(&entries, &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert_eq!(result.diagnostics.len(), 2);

    assert!(result.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.kind,
        Kind::Resolution { code: None }
    ) && diagnostic.location.module.source()
        == entry
        && &project.source(&entry).expect("entry source").text
            [diagnostic.location.range[0]..diagnostic.location.range[1]]
            == "'./absent'"));

    let linted = project.lint(&entries, &options())?;
    assert_eq!(linted.completion, result.completion);
    assert_eq!(linted.modules, result.modules);

    assert_eq!(
        linted.diagnostics,
        result
            .diagnostics
            .into_iter()
            .map(Diagnostic::from)
            .collect::<Vec<_>>()
    );

    Ok(())
}

#[test]
fn invalid_roblox_metadata_is_explicitly_incomplete() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/invalid/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/invalid/entry.luau"),
    )?;

    let mut project = project();

    project.change(Change::Overlay {
        path: roblox::cache_directory()?.join("metadata.json"),
        text: Some("{}".to_owned()),
    })?;

    let result = project.check(&[Entry::new(entry)], &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == Kind::Analysis { code: None })
    );

    Ok(())
}

#[test]
fn broken_dependency_reports_syntax_without_a_generic_import_failure() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let dependency = directory.file("dependency.luau", "local =")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/dependency/first.luau"),
    )?;

    let result = project().check(&[Entry::new(entry)], &options())?;
    assert_eq!(result.completion, Completion::Complete);

    assert!(
        result
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == dependency.as_path()
                    && matches!(diagnostic.kind, Kind::Syntax { .. })
            )
    );

    assert!(
        !result
            .diagnostics
            .iter()
            .any(|diagnostic| matches!(diagnostic.kind, Kind::Resolution { .. }))
    );

    Ok(())
}

#[test]
fn malformed_require_has_one_resolution_error() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/malformed/entry.luau"),
    )?;

    let result = project().check(&[Entry::new(entry)], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.diagnostics.len(), 1);

    assert!(matches!(
        result.diagnostics[0].kind,
        Kind::Resolution { .. }
    ));

    Ok(())
}

#[test]
fn executable_configuration_and_module_directives_control_modes() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    fs::remove_file(directory.path.join(".luaurc"))?;

    directory.file(
        ".config.luau",
        include_str!("../fixtures/checking/executable/.config.luau"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/executable/entry.luau"),
    )?;

    let mut project = project();

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(include_str!("../fixtures/checking/executable/input.luau").to_owned()),
    })?;

    assert_ne!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn declarations_can_build_on_prior_files_and_be_removed() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let configuration = directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/composition/instar.toml"),
    )?;

    directory.file(
        "types.luau",
        include_str!("../fixtures/checking/composition/types.luau"),
    )?;

    directory.file(
        "globals.luau",
        include_str!("../fixtures/checking/composition/globals.luau"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/declarations/entry.luau"),
    )?;

    let mut project = project();

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: configuration,
        text: Some(
            include_str!("../fixtures/checking/declarations/isolated/instar.toml").to_owned(),
        ),
    })?;

    assert_ne!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn explicit_declarations_preserve_distinct_mapped_contexts() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/declarations/instar.toml"),
    )?;

    directory.file(
        "globals.luau",
        include_str!("../fixtures/checking/explicit/globals.luau"),
    )?;

    let caller = directory.file(
        "caller.luau",
        include_str!("../fixtures/checking/explicit/caller.luau"),
    )?;

    directory.file("number.luau", "return 1")?;

    directory.file(
        "string.luau",
        include_str!("../fixtures/checking/explicit/string.luau"),
    )?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/checking/explicit/sourcemap.json"),
    )?;

    let mut project = project();
    let contexts = project.contexts(&caller)?;
    assert_eq!(contexts.len(), 2);

    let entries = contexts
        .iter()
        .map(|module| Entry {
            source: module.source.clone(),
            context: Some(module.identity.clone()),
        })
        .collect::<Vec<_>>();

    let result = project.check(&entries, &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.modules.len(), 4);
    assert_ne!(result.diagnostics, Vec::new());

    assert!(result.diagnostics.iter().all(|diagnostic| matches!(
        &diagnostic.location.module, Origin::Module(module) if module.identity == contexts[1].identity
    )));

    Ok(())
}

#[test]
fn roblox_security_profiles_and_metadata_control_native_types() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let configuration = directory.file(
        "instar.toml",
        include_str!("../fixtures/checking/roblox/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/roblox/entry.luau"),
    )?;

    let entries = [Entry::new(entry)];
    let mut project = project();
    let plugin = project.check(&entries, &options())?;
    assert_eq!(plugin.completion, Completion::Complete);
    assert_eq!(plugin.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration,
        text: Some(include_str!("../fixtures/checking/roblox/input.toml").to_owned()),
    })?;

    let game = project.check(&entries, &options())?;
    assert_eq!(game.completion, Completion::Complete);
    assert_eq!(game.diagnostics.len(), 1);
    assert!(game.diagnostics[0].message.contains("plugin"));
    let metadata = roblox::cache_directory()?.join("none.d.luau");

    project.change(Change::Overlay {
        path: metadata,
        text: Some(include_str!("../fixtures/checking/roblox/definition.luau").to_owned()),
    })?;

    let mismatched = project.check(&entries, &options())?;

    assert_eq!(
        mismatched.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(
        mismatched
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("do not match"))
    );

    Ok(())
}

#[test]
fn nonstrict_keeps_native_unknown_global_errors() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        ".luaurc",
        include_str!("../fixtures/checking/native/nonstrict.json"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/nonstrict/entry.luau"),
    )?;

    let result = project().check(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(result.completion, Completion::Complete);

    assert!(
        result
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.location.module.source() == entry.as_path()
                    && matches!(diagnostic.kind, Kind::Type { .. })
            )
    );

    Ok(())
}
