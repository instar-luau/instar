//! Synthetic native checking across project snapshots.

use instar_check::{Completion, Kind, Options, Reason};

use instar_core::{
    checking::{Entry, Origin},
    project::{Change, Project},
};

use serde_json::json;

use std::{
    fs, io,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Directory(PathBuf);

impl Directory {
    fn new() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "instar-checking-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        fs::create_dir(&path)?;
        let directory = Self(path);
        directory.file(".luaurc", r#"{"languageMode":"strict"}"#)?;

        Ok(directory)
    }

    fn file(&self, name: &str, text: &str) -> io::Result<PathBuf> {
        let path = self.0.join(name);
        fs::create_dir_all(path.parent().expect("fixture parent"))?;
        fs::write(&path, text)?;

        Ok(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove checker fixture");
    }
}

fn options() -> Options {
    Options::new(Duration::from_secs(5))
}

fn project() -> Project {
    Project::new(Duration::from_secs(2))
}

#[test]
fn imported_types_and_source_updates() -> io::Result<()> {
    let directory = Directory::new()?;

    let dependency = directory.file(
        "dependency.luau",
        "export type Value = {number: number}\nreturn {number = 1}",
    )?;

    let entry = directory.file("entry.luau", "local dependency = require('./dependency')\nlocal value: dependency.Value = dependency\nlocal number: string = value.number\nreturn number")?;
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
        text: Some("export type Value = {number: string}\nreturn {number = 'one'}".to_owned()),
    })?;

    let updated = project.check(&entries, &options())?;
    assert_eq!(updated.completion, Completion::Complete);
    assert!(updated.diagnostics.is_empty(), "{:?}", updated.diagnostics);

    Ok(())
}

#[test]
fn dependency_errors_are_not_filtered_or_repeated() -> io::Result<()> {
    let directory = Directory::new()?;
    directory.file("instar.toml", "[check]\nexclude = ['dependency.luau']")?;

    let dependency = directory.file(
        "dependency.luau",
        "local number: number = 'wrong'\nreturn number",
    )?;

    let first = directory.file("first.luau", "return require('./dependency')")?;
    let second = directory.file("second.luau", "return require('./dependency')")?;
    let mut project = project();
    let single = project.check(&[Entry::new(first.clone())], &options())?;
    let multiple = project.check(&[Entry::new(first), Entry::new(second)], &options())?;

    let dependency_errors = |result: &instar_check::Result<Origin>| {
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
    let directory = Directory::new()?;

    let first = directory.file(
        "first.luau",
        "local second = require('./second')\nreturn {second = second}",
    )?;

    directory.file(
        "second.luau",
        "local first = require('./first')\nreturn {first = first}",
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
    let directory = Directory::new()?;

    directory.file(
        "instar.toml",
        "[environment]\ndefinitions = ['globals.luau']",
    )?;

    let definition = directory.file("globals.luau", "declare configured: number")?;

    let entry = directory.file(
        "entry.luau",
        "local number: number = configured\nreturn number",
    )?;

    directory.file("isolated/instar.toml", "[environment]\ndefinitions = []")?;
    let isolated = directory.file("isolated/entry.luau", "return configured")?;
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
        text: Some("declare configured: string".to_owned()),
    })?;

    assert_ne!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: definition.clone(),
        text: Some("declare configured:".to_owned()),
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
        text: Some("declare configured: number".to_owned()),
    })?;

    assert_eq!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn native_modes_and_lint_ownership() -> io::Result<()> {
    let directory = Directory::new()?;
    let configuration = directory.0.join(".luaurc");

    let entry = directory.file(
        "entry.luau",
        "local unused = 1\nlocal function value(number: number) return number end\nreturn value()",
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
        text: Some(r#"{"languageMode":"nonstrict"}"#.to_owned()),
    })?;

    let nonstrict = project.check(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(nonstrict.diagnostics, Vec::new());

    project.change(Change::Overlay {
        path: configuration,
        text: Some(r#"{"languageMode":"nocheck"}"#.to_owned()),
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
    let directory = Directory::new()?;
    let entry = directory.file("entry.luau", "return require('./missing')")?;
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
    let directory = Directory::new()?;
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
fn roblox_without_native_setup_is_explicitly_incomplete() -> io::Result<()> {
    let directory = Directory::new()?;
    directory.file("instar.toml", "[roblox]\nenabled = true")?;
    let entry = directory.file("entry.luau", "return game")?;
    let result = project().check(&[Entry::new(entry)], &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Unsupported)
    );

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == Kind::Unsupported)
    );

    Ok(())
}

#[test]
fn broken_dependency_reports_syntax_without_a_generic_import_failure() -> io::Result<()> {
    let directory = Directory::new()?;
    let dependency = directory.file("dependency.luau", "local =")?;
    let entry = directory.file("entry.luau", "return require('./dependency')")?;
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
    let directory = Directory::new()?;
    let entry = directory.file("entry.luau", "return require()")?;
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
    let directory = Directory::new()?;
    fs::remove_file(directory.0.join(".luaurc"))?;
    directory.file(".config.luau", "return {languageMode = 'strict'}")?;
    let entry = directory.file("entry.luau", "--!nocheck\nreturn unknown")?;
    let mut project = project();

    assert_eq!(
        project
            .check(&[Entry::new(entry.clone())], &options())?
            .diagnostics,
        Vec::new()
    );

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some("--!strict\nreturn unknown".to_owned()),
    })?;

    assert_ne!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn declarations_can_build_on_prior_files_and_be_removed() -> io::Result<()> {
    let directory = Directory::new()?;

    let configuration = directory.file(
        "instar.toml",
        "[environment]\ndefinitions = ['types.luau', 'globals.luau']",
    )?;

    directory.file("types.luau", "export type Value = number")?;
    directory.file("globals.luau", "declare configured: Value")?;

    let entry = directory.file(
        "entry.luau",
        "local number: number = configured\nreturn number",
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
        text: Some("[environment]\ndefinitions = []".to_owned()),
    })?;

    assert_ne!(
        project.check(&[Entry::new(entry)], &options())?.diagnostics,
        Vec::new()
    );

    Ok(())
}

#[test]
fn explicit_declarations_preserve_distinct_mapped_contexts() -> io::Result<()> {
    let directory = Directory::new()?;

    directory.file(
        "instar.toml",
        "[environment]\ndefinitions = ['globals.luau']",
    )?;

    directory.file("globals.luau", "declare script: any")?;
    let caller = directory.file("caller.luau", "local imported = require(script.Parent.Target)\nlocal number: number = imported\nreturn number")?;
    directory.file("number.luau", "return 1")?;
    directory.file("string.luau", "return 'one'")?;

    directory.file(
        "sourcemap.json",
        &json!({
            "name":"Place", "className":"DataModel", "children":[
                {"name":"One", "className":"Folder", "children":[
                    {"name":"Caller", "className":"ModuleScript", "filePaths":["caller.luau"]},
                    {"name":"Target", "className":"ModuleScript", "filePaths":["number.luau"]}
                ]},
                {"name":"Two", "className":"Folder", "children":[
                    {"name":"Caller", "className":"ModuleScript", "filePaths":["caller.luau"]},
                    {"name":"Target", "className":"ModuleScript", "filePaths":["string.luau"]}
                ]}
            ]
        })
        .to_string(),
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
fn roblox_security_filtering_is_not_silently_ignored() -> io::Result<()> {
    let directory = Directory::new()?;
    directory.file("instar.toml", "[environment]\ndefinitions = ['globals.luau']\n[roblox]\nenabled = true\nsecurity = 'plugin'")?;
    directory.file("globals.luau", "declare game: any")?;
    let entry = directory.file("entry.luau", "return game")?;
    let result = project().check(&[Entry::new(entry)], &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Unsupported)
    );

    assert!(
        result
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.kind == Kind::Unsupported)
    );

    Ok(())
}

#[test]
fn nonstrict_keeps_native_unknown_global_errors() -> io::Result<()> {
    let directory = Directory::new()?;
    directory.file(".luaurc", r#"{"languageMode":"nonstrict"}"#)?;
    let entry = directory.file("entry.luau", "local unused = 1\nreturn undeclared")?;
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
