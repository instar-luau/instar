//! Native and Instar lint policies over shared contextual project sources.

use std::{io, time::Duration};

use instar_core::{
    analysis::{Entry, Origin},
    project::Change,
};

use instar_lint::{Completion, Kind, Level, Options, Reason, Rule};

use crate::support::{Directory, options, project};

fn rule(result: &instar_lint::Result<Origin>, name: Rule) -> bool {
    result
        .diagnostics
        .iter()
        .any(|diagnostic| diagnostic.kind == Kind::Rule(name))
}

#[test]
fn native_and_instar_policies_are_independent_and_checker_stays_clean() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file("instar.toml", "[lint.unused_variable]\nlevel='info'")?;
    let entry = directory.file("entry.luau", "local unused=1\nreturn 2")?;
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
        path: directory.path.join(".luaurc"),
        text: Some(
            r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true},"lintErrors":true}"#
                .to_owned(),
        ),
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
        text: Some("--!nolint LocalUnused\nlocal unused=1\nreturn 2".to_owned()),
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
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file("instar.toml", "include=['entry.luau']")?;
    let entry = directory.file("entry.luau", "return require('./dependency')")?;

    let dependency = directory.file(
        "dependency.luau",
        "local unused=1\nreturn require('./missing')",
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
                        Kind::Analysis(instar_analysis::Kind::Resolution { .. })
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
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file(".luaurc", r#"{"lint":{"*":false}}"#)?;
    directory.file("instar.toml", "[lint.restricted_module_paths]\nlevel='deny'\n[lint.restricted_module_paths.paths]\n'./dependency'='blocked'\n[lint.non_const_require]\nlevel='warn'")?;
    directory.file("dependency.luau", "return 1")?;

    let entry = directory.file(
        "entry.luau",
        "local loader=require\nlocal path='./'..'dependency'\nreturn loader(path)",
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
        text: Some(
            "local require=function(value) return value end\nreturn require('./dependency')"
                .to_owned(),
        ),
    })?;

    let shadowed = project.lint(&[Entry::new(entry)], &options())?;
    assert!(!rule(&shadowed, Rule::RestrictedModulePaths));
    assert!(!rule(&shadowed, Rule::NonConstRequire));

    Ok(())
}

#[test]
fn syntax_only_rules_do_not_load_declaration_environments() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file(".luaurc", r#"{"lint":{"*":false}}"#)?;

    directory.file(
        "instar.toml",
        "[[environment]]\nnamespace='@custom'\ndefinitions=['missing.luau']",
    )?;

    let entry = directory.file("entry.luau", "return 1/0")?;
    let result = project().lint(&[Entry::new(entry)], &options())?;
    assert_eq!(result.completion, Completion::Complete);
    assert!(rule(&result, Rule::DivideByZero));

    assert!(!result.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.kind,
        Kind::Analysis(instar_analysis::Kind::Analysis { .. })
    )));

    Ok(())
}

#[test]
fn inferred_any_uses_native_types_and_per_module_selection() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file(
        "instar.toml",
        "[lint.implicit_any_parameter]\nlevel='warn'\n[lint.implicit_any_local]\nlevel='warn'",
    )?;

    directory.file(
        "dependency/instar.toml",
        "[lint.implicit_any_parameter]\nlevel='allow'\n[lint.implicit_any_local]\nlevel='allow'",
    )?;

    directory.file(
        "dependency/.luaurc",
        r#"{"languageMode":"nocheck","lint":{"*":false}}"#,
    )?;

    directory.file("dependency/init.luau", "return 1")?;
    let entry = directory.file("entry.luau", "local value=1\nlocal function f(parameter:number) return parameter end\nreturn value,f,require('./dependency')")?;
    let mut project = project();
    let strict = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(strict.completion, Completion::Complete);
    assert!(!rule(&strict, Rule::ImplicitAnyLocal));
    assert!(!rule(&strict, Rule::ImplicitAnyParameter));

    project.change(Change::Overlay {
        path: directory.path.join(".luaurc"),
        text: Some(r#"{"languageMode":"nonstrict","lint":{"*":false}}"#.to_owned()),
    })?;

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some(
            "local function f(parameter) return parameter end\nlocal value=f(1)\nreturn value,f"
                .to_owned(),
        ),
    })?;

    let inferred = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(inferred.completion, Completion::Complete);
    assert!(!rule(&inferred, Rule::ImplicitAnyParameter));
    project.change(Change::Overlay { path: entry.clone(), text: Some("local callback: (any)->any = function(parameter) return parameter end\nlocal value=callback(1)\nreturn value,callback".to_owned()) })?;
    let nonstrict = project.lint(&[Entry::new(entry.clone())], &options())?;
    assert_eq!(nonstrict.completion, Completion::Complete);
    assert!(rule(&nonstrict, Rule::ImplicitAnyParameter));

    project.change(Change::Overlay {
        path: entry.clone(),
        text: Some("--!nocheck\nlocal value=1\nreturn value".to_owned()),
    })?;

    let nocheck = project.lint(&[Entry::new(entry)], &options())?;
    assert_eq!(nocheck.completion, Completion::Incomplete(Reason::Analysis));

    assert!(nocheck.diagnostics.iter().any(|diagnostic| matches!(
        diagnostic.kind,
        Kind::Analysis(instar_analysis::Kind::Analysis { .. })
    )));

    Ok(())
}

#[test]
fn missing_and_invalid_definitions_remain_explicit_with_syntax_findings() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file(
        "instar.toml",
        "[[environment]]\nnamespace='@custom'\ndefinitions=['globals.luau']",
    )?;

    let entry = directory.file("entry.luau", "local unused=1\nreturn 1/0")?;
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
        Kind::Analysis(instar_analysis::Kind::Analysis { .. })
    )));

    let definition = directory.path.join("globals.luau");

    project.change(Change::Overlay {
        path: definition.clone(),
        text: Some("declare configured:".to_owned()),
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
                        Kind::Analysis(instar_analysis::Kind::Syntax { .. })
                    )
            )
    );

    project.change(Change::Overlay {
        path: definition,
        text: Some("declare configured: number".to_owned()),
    })?;

    assert_eq!(
        project.lint(&[Entry::new(entry)], &options())?.completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn invalid_roblox_metadata_keeps_known_lint_findings() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file("instar.toml", "[roblox]\nenabled=true\nsecurity='plugin'")?;

    let entry = directory.file("entry.luau", "return Color3.new(255,0,0)")?;
    let mut project = project();

    project.change(Change::Overlay {
        path: instar_core::roblox::cache_directory()?.join("metadata.json"),
        text: Some("{}".to_owned()),
    })?;

    let result = project.lint(&[Entry::new(entry)], &options())?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(Reason::Environment)
    );

    assert!(rule(&result, Rule::RobloxIncorrectColor3NewBounds));

    assert!(result.diagnostics.iter().any(|diagnostic| diagnostic.kind
        == Kind::Analysis(instar_analysis::Kind::Analysis { code: None })));

    Ok(())
}

#[test]
fn source_updates_and_contexts_keep_exact_revision_identity() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

    directory.file(
        "instar.toml",
        "[roblox]\nenabled=true\n[[environment]]\nnamespace='@custom'\ndefinitions=['globals.luau']",
    )?;

    directory.file("globals.luau", "declare script: any")?;
    let shared = directory.file("shared.luau", "return 1/0")?;
    directory.file("sourcemap.json", r#"{"name":"Game","className":"DataModel","children":[{"name":"First","className":"ModuleScript","filePaths":["shared.luau"]},{"name":"Second","className":"ModuleScript","filePaths":["shared.luau"]}]}"#)?;
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
        text: Some("return 1/1".to_owned()),
    })?;

    let updated = project.lint(&entries, &options())?;
    assert!(!rule(&updated, Rule::DivideByZero));
    assert_ne!(project.source(&entries[0].source)?.revision, revision);

    Ok(())
}

#[test]
fn cancellation_and_timeout_are_explicit() -> io::Result<()> {
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"LocalUnused":true}}"#,
    ))?;

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
    let directory = Directory::new(Some(
        r#"{"languageMode":"strict","lint":{"*":false,"TableOperations":true}}"#,
    ))?;

    let entry = directory.file("entry.luau", "return require('./absent')")?;
    let mut project = project();
    let entries = [Entry::new(entry.clone())];

    for source in [
        "return require('./absent')",
        "require()require()",
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
                .map(instar_lint::Diagnostic::from)
                .collect::<Vec<_>>()
        );
    }

    project.change(Change::Overlay {
        path: entry,
        text: Some("return 1".to_owned()),
    })?;

    project.change(Change::Overlay {
        path: directory.path.join("instar.toml"),
        text: Some("[[environment]]\nnamespace='@custom'\ndefinitions=['globals.luau']".to_owned()),
    })?;

    for declaration in ["declare configured:", "declare configured: MissingType"] {
        project.change(Change::Overlay {
            path: directory.path.join("globals.luau"),
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
                .map(instar_lint::Diagnostic::from)
                .collect::<Vec<_>>()
        );
    }

    Ok(())
}
