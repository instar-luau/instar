//! Synthetic native checking across project snapshots.

use std::{
    fs, io,
    time::{Duration, Instant},
};

use instar_shared::{Completion, DiagnosticCategory, Options, Reason, Severity};

use instar_project::{
    CheckReport, CheckRequest, Input, InputIdentity, InputState, Invocation, LintRequest,
    SourceOrigin, roblox_cache_directory,
};

use crate::support::{Directory, complete, options, project, replace};

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

    let project = project();
    let entries = [Input::Path(entry.clone())];

    let first = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(first.completion, Completion::Complete);

    assert!(
        first
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Type)
    );

    assert!(first.findings.iter().all(|diagnostic| {
        diagnostic.primary.span.end
            <= project
                .snapshot()
                .source_at(&entry)
                .expect("source")
                .text
                .len()
    }));

    replace(
        &project,
        &dependency,
        include_str!("../fixtures/checking/imported/replacement.luau"),
    )?;

    let updated = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(updated.completion, Completion::Complete);
    assert!(updated.findings.is_empty(), "{:?}", updated.findings);

    Ok(())
}

#[test]
fn dependency_type_findings_are_reported_without_dropping_analysis() -> io::Result<()> {
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

    let project = project();

    let single = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(first.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let multiple = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(first), Input::Path(second)],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let dependency_errors = |result: &CheckReport| {
        result
            .findings
            .iter()
            .filter(|diagnostic| {
                project
                    .snapshot()
                    .origin(diagnostic.primary.source.identity)
                    .and_then(SourceOrigin::path)
                    .expect("finding origin")
                    == dependency.as_path()
            })
            .count()
    };

    assert_eq!(dependency_errors(&single), 1);
    assert_eq!(dependency_errors(&multiple), 1);
    assert_eq!(single.findings.len(), 1);
    assert_eq!(multiple.findings.len(), 1);
    assert_eq!(single.findings[0].identity.inputs(), single.sources);
    assert_eq!(multiple.findings[0].identity.inputs(), multiple.sources);

    assert_eq!(
        multiple.findings[0].identity.producer(),
        single.findings[0].identity.producer()
    );

    let mut captured = multiple.findings[0].clone();
    captured.identity = single.findings[0].identity.clone();
    assert_eq!(captured, single.findings[0]);
    assert_eq!(single.findings[0].category, DiagnosticCategory::Type);
    assert_eq!(single.findings[0].severity, Severity::Error);
    assert_eq!(single.findings[0].identity.producer(), "luau");
    assert_eq!(single.sources.len(), 2);
    assert_eq!(multiple.sources.len(), 3);
    assert_eq!(single.completion, Completion::Complete);
    assert_eq!(multiple.completion, Completion::Complete);

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(dependency)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
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

    let project = project();

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    assert_ne!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(isolated)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(
        &project,
        &definition,
        include_str!("../fixtures/checking/declarations/definition.luau"),
    )?;

    assert_ne!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(
        &project,
        &definition,
        include_str!("../fixtures/checking/declarations/invalid.luau"),
    )?;

    let invalid = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert!(invalid.findings.iter().any(|diagnostic| {
        project
            .snapshot()
            .origin(diagnostic.primary.source.identity)
            .and_then(SourceOrigin::path)
            .expect("finding origin")
            == entry
            && diagnostic.related.iter().any(|related| {
                project
                    .snapshot()
                    .origin(related.location.source.identity)
                    .and_then(SourceOrigin::path)
                    .expect("finding origin")
                    == definition
            })
            && diagnostic.category == DiagnosticCategory::Analysis
    }));

    replace(
        &project,
        &definition,
        include_str!("../fixtures/checking/declarations/globals.luau"),
    )?;

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    Ok(())
}

#[test]
fn native_modes_and_lint_ownership() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let configuration = directory.path().join(".luaurc");

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/native/entry.luau"),
    )?;

    let project = project();

    assert_ne!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/checking/native/nonstrict.json"),
    )?;

    let nonstrict = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(nonstrict.findings, Vec::new());

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/checking/native/nocheck.json"),
    )?;

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(&project, &entry, "local =")?;

    assert!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings
        .iter()
        .any(|diagnostic| diagnostic.category == DiagnosticCategory::Syntax)
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

    let project = project();

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry)],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.findings.len(), 1);

    assert!(result.findings[0].category == DiagnosticCategory::Resolution);

    assert!(result.findings[0].primary.span.start > 0);

    Ok(())
}

#[test]
fn interruption_is_not_success_and_can_be_retried() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let entry = directory.file("entry.luau", "return 1")?;
    let project = project();
    let cancelled = options();
    cancelled.cancellation.cancel();

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            cancelled.clone()
        ))
        .expect_err("cancelled invocation")
        .completion,
        Completion::Cancelled
    );

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            Options::new(Some(Instant::now() + Duration::ZERO)).clone()
        ))
        .expect_err("expired invocation")
        .completion,
        Completion::TimedOut
    );

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .completion,
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

    let project = project();
    let entries = [Input::Path(entry.clone())];

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert_eq!(result.findings.len(), 2);

    assert!(result.findings.iter().any(|diagnostic| {
        diagnostic.category == DiagnosticCategory::Resolution
            && project
                .snapshot()
                .origin(diagnostic.primary.source.identity)
                .and_then(SourceOrigin::path)
                .expect("finding origin")
                == entry
            && &project.snapshot().source_at(&entry).expect("source").text
                [diagnostic.primary.span.start..diagnostic.primary.span.end]
                == "'./absent'"
    }));

    let linted = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(linted.completion, result.completion);
    assert_eq!(linted.sources, result.sources);

    assert_eq!(result.findings.len(), 2);
    assert_eq!(linted.findings.len(), 3);

    let prerequisites = linted
        .findings
        .iter()
        .filter(|diagnostic| diagnostic.identity.producer() == "lint")
        .collect::<Vec<_>>();

    assert_eq!(prerequisites.len(), 1);
    assert_eq!(prerequisites[0].code, "lint.prerequisite");
    assert_eq!(prerequisites[0].category, DiagnosticCategory::Analysis);
    assert_eq!(prerequisites[0].severity, Severity::Error);

    assert_eq!(
        prerequisites[0].primary.source,
        project
            .snapshot()
            .source_at(&entry)
            .expect("source")
            .version,
    );

    assert_eq!(
        prerequisites[0].identity.inputs(),
        &[prerequisites[0].primary.source]
    );

    assert!(prerequisites[0].message.contains("unscoped_variables"));
    assert!(prerequisites[0].message.contains("UnavailableEnvironment"));

    assert_eq!(
        linted
            .findings
            .into_iter()
            .filter(|diagnostic| { diagnostic.identity.producer() != "lint" })
            .collect::<Vec<_>>(),
        result.findings,
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

    let project = project();

    replace(
        &project,
        &roblox_cache_directory()?.join("metadata.json"),
        "{}",
    )?;

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry)],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        result.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(
        result
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Analysis)
    );

    Ok(())
}

#[test]
fn broken_dependency_reports_failure_on_the_selected_importer() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    let dependency = directory.file("dependency.luau", "local =")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/dependency/first.luau"),
    )?;

    let project = project();

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);

    assert!(result.findings.iter().any(|diagnostic| {
        project
            .snapshot()
            .origin(diagnostic.primary.source.identity)
            .and_then(SourceOrigin::path)
            .expect("finding origin")
            == entry.as_path()
            && diagnostic.category == DiagnosticCategory::Analysis
            && diagnostic.related.iter().any(|related| {
                project
                    .snapshot()
                    .origin(related.location.source.identity)
                    .and_then(SourceOrigin::path)
                    .expect("finding origin")
                    == dependency
            })
    }));

    assert!(
        !result
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Resolution)
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

    let project = project();

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry)],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.findings.len(), 1);

    assert!(result.findings[0].category == DiagnosticCategory::Resolution);

    Ok(())
}

#[test]
fn executable_configuration_and_module_directives_control_modes() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    fs::remove_file(directory.path().join(".luaurc"))?;

    directory.file(
        "instar.luau",
        include_str!("../fixtures/checking/executable/instar.luau"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/executable/entry.luau"),
    )?;

    let project = project();

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(
        &project,
        &entry,
        include_str!("../fixtures/checking/executable/input.luau"),
    )?;

    assert_ne!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
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

    let project = project();

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry.clone())],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
        Vec::new()
    );

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/checking/declarations/isolated/instar.toml"),
    )?;

    assert_ne!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .findings,
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

    let project = project();

    let initial = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(caller.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let source = initial
        .snapshot
        .source_at(&caller)
        .expect("caller")
        .version
        .identity;

    let contexts = initial
        .modules
        .iter()
        .copied()
        .filter(|module| module.source == source)
        .collect::<Vec<_>>();

    assert_eq!(contexts.len(), 2);

    let entries = contexts
        .iter()
        .copied()
        .map(Input::Module)
        .collect::<Vec<_>>();

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);
    assert_eq!(result.modules.len(), 4);
    assert_ne!(result.findings, Vec::new());

    assert_eq!(result.module_findings.len(), result.findings.len());

    assert!(
        result
            .module_findings
            .iter()
            .all(|finding| finding.module == contexts[1])
    );

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

    let entries = [Input::Path(entry)];
    let project = project();

    let plugin = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(plugin.completion, Completion::Complete);
    assert_eq!(plugin.findings, Vec::new());

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/checking/roblox/input.toml"),
    )?;

    let game = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(game.completion, Completion::Complete);
    assert_eq!(game.findings.len(), 1);
    assert!(game.findings[0].message.contains("plugin"));
    let metadata = roblox_cache_directory()?.join("none.d.luau");

    replace(
        &project,
        &metadata,
        include_str!("../fixtures/checking/roblox/definition.luau"),
    )?;

    let mismatched = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        mismatched.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(
        mismatched
            .findings
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

    let project = project();

    let result = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);

    assert!(result.findings.iter().any(|diagnostic| {
        project
            .snapshot()
            .origin(diagnostic.primary.source.identity)
            .and_then(SourceOrigin::path)
            .expect("finding origin")
            == entry.as_path()
            && diagnostic.category == DiagnosticCategory::Type
    }));

    Ok(())
}

#[test]
fn excluded_dependency_environment_failures_are_attributed_and_recover() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/selection/instar.toml"),
    )?;

    directory.file(
        "dependency/instar.toml",
        include_str!("../fixtures/selection/dependency.toml"),
    )?;

    directory.file("dependency/init.luau", "return 1")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/checking/dependency/first.luau"),
    )?;

    let definition = directory.path().join("dependency").join("missing.luau");
    let entries = [Input::Path(entry.clone())];
    let project = project();

    let checked = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let linted = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        checked.completion,
        Completion::Incomplete(vec![
            Reason::UnavailableEnvironment,
            Reason::UnavailableSemantics
        ])
    );

    assert_eq!(linted.completion, checked.completion);
    assert_eq!(checked.sources.len(), 2);
    assert_eq!(linted.sources, checked.sources);

    let dependency = directory.path().join("dependency/init.luau");
    let snapshot = project.snapshot();

    for (inputs, findings, count) in [
        (&checked.inputs, &checked.findings, 2),
        (&linted.inputs, &linted.findings, 6),
    ] {
        assert_eq!(findings.len(), count);

        assert!(inputs.iter().any(|input| {
            input.identity == InputIdentity::Path(definition.clone())
                && input.state == InputState::Missing
        }));

        assert!(findings.iter().all(|diagnostic| {
            diagnostic.category == DiagnosticCategory::Analysis
                && diagnostic.severity == Severity::Error
                && matches!(
                    snapshot.origin(diagnostic.primary.source.identity).and_then(SourceOrigin::path),
                    Some(path) if path == entry || path == dependency
                )
        }));

        let prerequisites = findings
            .iter()
            .filter(|diagnostic| diagnostic.identity.producer() == "project")
            .collect::<Vec<_>>();

        assert_eq!(prerequisites.len(), 2);

        let unavailable = prerequisites
            .iter()
            .find(|diagnostic| diagnostic.code == "environment.unavailable")
            .expect("dependency environment finding");

        assert_eq!(
            snapshot
                .origin(unavailable.primary.source.identity)
                .and_then(SourceOrigin::path),
            Some(dependency.as_path()),
        );

        assert!(
            unavailable
                .message
                .contains(&definition.display().to_string()),
            "{unavailable:?}"
        );

        assert!(unavailable.related.is_empty());

        let importer = prerequisites
            .iter()
            .find(|diagnostic| diagnostic.code == "analysis.dependency")
            .expect("importer prerequisite finding");

        assert_eq!(
            snapshot
                .origin(importer.primary.source.identity)
                .and_then(SourceOrigin::path),
            Some(entry.as_path()),
        );

        let mut expected = vec![unavailable.message.as_str()];

        if count == 6 {
            expected.extend([
                "implicit_any_local has incomplete coverage: UnavailableEnvironment",
                "unscoped_variables has incomplete coverage: UnavailableEnvironment",
            ]);
        }

        let mut messages = importer
            .related
            .iter()
            .map(|related| related.message.as_str())
            .collect::<Vec<_>>();

        expected.sort_unstable();
        messages.sort_unstable();
        assert_eq!(messages, expected, "{importer:?}");

        assert!(
            importer
                .related
                .iter()
                .all(|related| related.location == unavailable.primary)
        );
    }

    replace(
        &project,
        &definition,
        include_str!("../fixtures/checking/declarations/globals.luau"),
    )?;

    assert_eq!(
        complete(project.check(
            CheckRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..CheckRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .completion,
        Completion::Complete
    );

    assert_eq!(
        complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..LintRequest::default()
            },
            options()
        ))
        .map_err(io::Error::from)?
        .completion,
        Completion::Complete
    );

    Ok(())
}

#[test]
fn dependency_failures_are_isolated_to_the_target_module_context() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/checking/imported/strict.json"
    )))?;

    directory.file("instar.toml", "linting.selected = ['divide_by_zero']")?;

    directory.file(
        "sourcemap.json",
        r#"{
            "name": "Game", "className": "DataModel", "children": [
                {"name": "First", "className": "Folder", "children": [
                    {"name": "Caller", "className": "ModuleScript", "filePaths": ["first.luau"]},
                    {"name": "Shared", "className": "ModuleScript", "filePaths": ["shared.luau"]}
                ]},
                {"name": "Second", "className": "Folder", "children": [
                    {"name": "Caller", "className": "ModuleScript", "filePaths": ["second.luau"]},
                    {"name": "Shared", "className": "ModuleScript", "filePaths": ["shared.luau"]},
                    {"name": "Target", "className": "ModuleScript", "filePaths": ["target.luau"]}
                ]}
            ]
        }"#,
    )?;

    let first = directory.file("first.luau", "return require(script.Parent.Shared)")?;
    let second = directory.file("second.luau", "return require(script.Parent.Shared)")?;
    let shared = directory.file("shared.luau", "return require(script.Parent.Target)")?;
    directory.file("target.luau", "return 1")?;
    let project = project();
    let entries = [Input::Path(first.clone()), Input::Path(second.clone())];

    let checked = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let linted = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    for (snapshot, findings) in [
        (&checked.snapshot, &checked.module_findings),
        (&linted.snapshot, &linted.module_findings),
    ] {
        let first_source = snapshot.source_at(&first).expect("first importer").version;

        let second_source = snapshot
            .source_at(&second)
            .expect("second importer")
            .version;

        let shared_source = snapshot
            .source_at(&shared)
            .expect("shared dependency")
            .version;

        let prerequisites = findings
            .iter()
            .filter(|finding| finding.diagnostic.code == "analysis.dependency")
            .collect::<Vec<_>>();

        assert_eq!(prerequisites.len(), 1, "{prerequisites:?}");
        assert_eq!(prerequisites[0].module.source, first_source.identity);
        assert_eq!(prerequisites[0].diagnostic.primary.source, first_source);

        assert!(
            prerequisites[0]
                .diagnostic
                .related
                .iter()
                .all(|related| related.location.source == shared_source)
        );

        assert!(!findings.iter().any(|finding| {
            finding.module.source == second_source.identity
                && finding.diagnostic.category == DiagnosticCategory::Analysis
        }));
    }

    Ok(())
}
