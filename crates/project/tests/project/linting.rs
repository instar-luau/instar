//! Native and Instar lint policies over shared contextual project sources.

use std::{
    io,
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use instar_luau::ImportOutcome;

use instar_shared::{
    Availability, Completion, Diagnostic, DiagnosticCategory, DiagnosticTag, EventPayload,
    FindingIdentity, Location, Options, Reason, Severity, SourceVersion, Span,
};

use instar_project::{
    CheckRequest, Input, InputIdentity, InputState, Invocation, LintReport, LintRequest,
    SourceOrigin, roblox_cache_directory,
};

use instar_lint::RuleIdentity;

use crate::support::{Directory, complete, options, project, replace};

fn rule(result: &LintReport, name: RuleIdentity) -> bool {
    result
        .findings
        .iter()
        .any(|diagnostic| diagnostic.code == name.name())
}

#[test]
fn native_and_instar_policies_are_independent_and_checker_stays_clean() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        r#"linting = { rules = { unused_variable = { enabled = false, severity = "Information", settings = { rule = "unused_variable", parameters = {} } } } }"#,
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/native/entry.luau"),
    )?;

    let project = project();
    let entries = [Input::Path(entry.clone())];

    let before = complete(project.check(
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

    assert_eq!(before.findings, Vec::new());

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

    assert_eq!(linted.completion, Completion::Complete);

    assert!(
        linted
            .findings
            .iter()
            .any(|diagnostic| diagnostic.identity.producer() == "luau"
                && diagnostic.code == "LocalUnused"
                && diagnostic.severity == Severity::Warning)
    );

    assert_eq!(linted.findings.len(), 1);
    assert!(!rule(&linted, RuleIdentity::UnusedVariable));

    assert_eq!(
        {
            let report = complete(project.check(
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
            (report.findings, report.sources, report.completion)
        },
        (
            before.findings.clone(),
            before.sources.clone(),
            before.completion.clone()
        )
    );

    replace(
        &project,
        &directory.path().join(".luaurc"),
        include_str!("../fixtures/linting/native/fatal.json"),
    )?;

    let fatal = complete(project.lint(
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

    assert!(
        fatal
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Lint
                && diagnostic.identity.producer() == "luau"
                && diagnostic.severity == Severity::Error)
    );

    replace(
        &project,
        &directory.path().join("instar.toml"),
        include_str!("../fixtures/linting/native/instar.toml"),
    )?;

    replace(
        &project,
        &entry,
        include_str!("../fixtures/linting/native/input.luau"),
    )?;

    let directive = complete(project.lint(
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

    assert!(
        !directive
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Lint
                && diagnostic.identity.producer() == "luau")
    );

    assert!(rule(&directive, RuleIdentity::UnusedVariable));
    assert_eq!(directive.findings.len(), 1);
    assert_eq!(directive.findings[0].severity, Severity::Information);

    Ok(())
}

#[test]
fn selection_reports_targets_and_attributes_dependency_failures() -> io::Result<()> {
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

    let project = project();
    let entries = [Input::Path(entry.clone())];

    let result = complete(project.lint(
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

    assert_eq!(result.sources.len(), 2);

    assert_eq!(
        result.sources,
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
        .sources
    );

    let findings_at = |path: &Path| {
        result
            .findings
            .iter()
            .filter(|diagnostic| {
                result
                    .snapshot
                    .origin(diagnostic.primary.source.identity)
                    .and_then(SourceOrigin::path)
                    == Some(path)
            })
            .count()
    };

    assert_eq!(findings_at(&entry), 1);
    assert_eq!(findings_at(&dependency), 2);
    assert_eq!(result.findings.len(), 3);

    assert!(!rule(&result, RuleIdentity::UnusedVariable));

    let native = result
        .findings
        .iter()
        .filter(|diagnostic| {
            diagnostic.category == DiagnosticCategory::Lint
                && diagnostic.identity.producer() == "luau"
        })
        .collect::<Vec<_>>();

    assert_eq!(native.len(), 1);
    assert_eq!(native[0].code, "LocalUnused");
    assert_eq!(native[0].severity, Severity::Warning);

    assert_eq!(
        result
            .snapshot
            .origin(native[0].primary.source.identity)
            .and_then(SourceOrigin::path),
        Some(dependency.as_path()),
    );

    assert_eq!(
        result
            .findings
            .iter()
            .filter(|diagnostic| {
                diagnostic.category == DiagnosticCategory::Resolution
                    && result
                        .snapshot
                        .origin(diagnostic.primary.source.identity)
                        .and_then(SourceOrigin::path)
                        == Some(dependency.as_path())
            })
            .count(),
        1
    );

    assert!(result.findings.iter().any(|diagnostic| {
        diagnostic.category == DiagnosticCategory::Analysis
            && diagnostic.related.iter().any(|related| {
                project
                    .snapshot()
                    .origin(related.location.source.identity)
                    .and_then(SourceOrigin::path)
                    .expect("finding origin")
                    == dependency
            })
    }));

    assert_eq!(
        complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(dependency)],
                    ..Invocation::default()
                },
                ..LintRequest::default()
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

    let project = project();

    let result = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);
    assert!(rule(&result, RuleIdentity::RestrictedModulePaths));
    assert!(!rule(&result, RuleIdentity::NonConstRequire));

    assert!(result.findings.iter().any(|diagnostic| diagnostic.code
        == RuleIdentity::RestrictedModulePaths.name()
        && &project.snapshot().source_at(&entry).expect("source").text
            [diagnostic.primary.span.start..diagnostic.primary.span.end]
            == "path"));

    replace(
        &project,
        &entry,
        include_str!("../fixtures/linting/shared/input.luau"),
    )?;

    let shadowed = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry)],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert!(!rule(&shadowed, RuleIdentity::RestrictedModulePaths));
    assert!(!rule(&shadowed, RuleIdentity::NonConstRequire));

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

    let result = complete(project().lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry)],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(result.completion, Completion::Complete);
    assert!(rule(&result, RuleIdentity::DivideByZero));

    assert!(
        !result
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Analysis)
    );

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

    let project = project();

    let strict = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(strict.completion, Completion::Complete);
    assert!(!rule(&strict, RuleIdentity::ImplicitAnyLocal));
    assert!(!rule(&strict, RuleIdentity::ImplicitAnyParameter));

    replace(
        &project,
        &directory.path().join(".luaurc"),
        include_str!("../fixtures/linting/inferred/nonstrict.json"),
    )?;

    replace(
        &project,
        &entry,
        include_str!("../fixtures/linting/inferred/input.luau"),
    )?;

    let inferred = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(inferred.completion, Completion::Complete);
    assert!(!rule(&inferred, RuleIdentity::ImplicitAnyParameter));

    replace(
        &project,
        &entry,
        include_str!("../fixtures/linting/inferred/replacement.luau"),
    )?;

    let limits = options();

    let nonstrict = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits.clone(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        {
            let report = complete(project.lint(
                LintRequest {
                    invocation: Invocation {
                        inputs: vec![Input::Path(entry.clone())],
                        ..Invocation::default()
                    },
                    ..LintRequest::default()
                },
                limits.clone(),
            ))
            .map_err(io::Error::from)?;
            (report.findings, report.sources, report.completion)
        },
        (
            nonstrict.findings.clone(),
            nonstrict.sources.clone(),
            nonstrict.completion.clone()
        )
    );

    assert_eq!(nonstrict.completion, Completion::Complete);
    assert!(rule(&nonstrict, RuleIdentity::ImplicitAnyParameter));

    replace(
        &project,
        &entry,
        include_str!("../fixtures/linting/inferred/invalid.luau"),
    )?;

    let nocheck = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        nocheck.completion,
        Completion::Incomplete(vec![
            Reason::UnavailableSemantics,
            Reason::FailedPrerequisite
        ])
    );

    assert_eq!(
        nocheck
            .availability
            .iter()
            .map(|availability| availability.rule)
            .collect::<Vec<_>>(),
        vec![
            RuleIdentity::ImplicitAnyLocal,
            RuleIdentity::ImplicitAnyParameter
        ],
    );

    assert!(nocheck.availability.iter().all(|availability| {
        availability.status == Availability::Partial(Reason::FailedPrerequisite)
    }));

    let version = SourceVersion {
        identity: nonstrict.sources[0].identity,
        revision: nonstrict.sources[0].revision + 1,
    };

    assert_eq!(nocheck.sources, vec![version]);

    let primary = Location {
        source: version,
        span: Span { start: 0, end: 0 },
    };

    assert_eq!(
        nocheck.findings,
        [
            RuleIdentity::ImplicitAnyLocal,
            RuleIdentity::ImplicitAnyParameter
        ]
        .into_iter()
        .map(|identity| {
            let message = format!("{identity} has incomplete coverage: FailedPrerequisite");

            Diagnostic {
                identity: FindingIdentity::new(
                    "lint",
                    vec![version],
                    format!("lint.prerequisite:{message}"),
                    primary,
                ),
                code: "lint.prerequisite".to_owned(),
                category: DiagnosticCategory::Analysis,
                severity: Severity::Error,
                message,
                primary,
                related: Vec::new(),
                tags: Vec::new(),
            }
        })
        .collect::<Vec<_>>(),
    );

    let checked = complete(project.check(
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

    assert_eq!(checked.completion, Completion::Complete);
    assert_eq!(checked.sources, nocheck.sources);
    assert!(checked.findings.is_empty());

    assert!(
        nocheck
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Analysis)
    );

    Ok(())
}

#[test]
fn missing_and_invalid_definitions_remain_explicit_with_syntax_findings() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file(
        "instar.toml",
        &include_str!("../fixtures/linting/missing/instar.toml")
            .replace("[\"globals.luau\"]", "[\"globals.luau\", \"second.luau\"]"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/missing/entry.luau"),
    )?;

    let project = project();

    let missing = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        missing.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(rule(&missing, RuleIdentity::DivideByZero));

    let definition = directory.path().join("globals.luau");
    let second = directory.path().join("second.luau");

    assert_eq!(
        missing
            .findings
            .iter()
            .filter(|diagnostic| diagnostic.code == "environment.unavailable")
            .count(),
        1,
    );

    assert!(missing.findings.iter().any(|diagnostic| {
        missing
            .snapshot
            .origin(diagnostic.primary.source.identity)
            .and_then(SourceOrigin::path)
            == Some(entry.as_path())
            && diagnostic.category == DiagnosticCategory::Analysis
            && diagnostic.code == "environment.unavailable"
            && diagnostic
                .message
                .contains(&definition.display().to_string())
            && diagnostic.message.contains(&second.display().to_string())
            && diagnostic.related.is_empty()
    }));

    assert!(missing.inputs.iter().any(|input| {
        input.identity == InputIdentity::Path(definition.clone())
            && input.state == InputState::Missing
    }));

    assert!(missing.inputs.iter().any(|input| {
        input.identity == InputIdentity::Path(second.clone()) && input.state == InputState::Missing
    }));

    replace(&project, &second, "")?;

    replace(
        &project,
        &definition,
        include_str!("../fixtures/linting/missing/invalid.luau"),
    )?;

    let invalid = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        invalid.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(invalid.findings.iter().any(|diagnostic| {
        diagnostic.related.iter().any(|related| {
            project
                .snapshot()
                .origin(related.location.source.identity)
                .and_then(SourceOrigin::path)
                .expect("finding origin")
                == definition
        }) && diagnostic.category == DiagnosticCategory::Analysis
    }));

    let invalid_definition = invalid
        .snapshot
        .source_at(&definition)
        .expect("invalid declaration");

    assert!(invalid.findings.iter().any(|diagnostic| {
        diagnostic.code == "environment.unavailable"
            && diagnostic.related.iter().any(|related| {
                related.location.source == invalid_definition.version
                    && related.location.span.start <= related.location.span.end
                    && related.location.span.end <= invalid_definition.text.len()
            })
            && diagnostic
                .identity
                .inputs()
                .contains(&invalid_definition.version)
    }));

    replace(
        &project,
        &definition,
        include_str!("../fixtures/linting/missing/definition.luau"),
    )?;

    assert_eq!(
        complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: vec![Input::Path(entry)],
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

    let project = project();

    let entries = [Input::Path(entry)];
    let metadata = roblox_cache_directory()?.join("metadata.json");

    let valid_metadata = project
        .snapshot()
        .source_at(&metadata)
        .expect("fixture metadata")
        .text
        .clone();

    let valid = complete(project.lint(
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

    assert!(
        valid.availability.iter().any(|availability| {
            availability.rule == RuleIdentity::RobloxIncorrectColor3NewBounds
                && availability.status == Availability::Available
        }),
        "{valid:#?}",
    );

    assert!(rule(&valid, RuleIdentity::RobloxIncorrectColor3NewBounds));
    assert!(rule(&valid, RuleIdentity::DivideByZero));

    replace(&project, &metadata, "{}")?;

    let result = complete(project.lint(
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
        result.completion,
        Completion::Incomplete(vec![Reason::UnavailableEnvironment])
    );

    assert!(rule(&result, RuleIdentity::DivideByZero));
    assert!(!rule(&result, RuleIdentity::RobloxIncorrectColor3NewBounds));

    assert!(result.availability.iter().any(|availability| {
        availability.rule == RuleIdentity::RobloxIncorrectColor3NewBounds
            && availability.status == Availability::Unavailable(Reason::UnavailableEnvironment)
    }));

    let metadata_source = result
        .snapshot
        .source_at(&metadata)
        .expect("invalid metadata");

    assert!(result.findings.iter().any(|diagnostic| {
        diagnostic.code == "assets.metadata"
            && diagnostic.primary.source == metadata_source.version
            && diagnostic.primary.span == Span { start: 0, end: 2 }
    }));

    assert!(result.findings.iter().any(|diagnostic| {
        diagnostic.code == "environment.unavailable"
            && diagnostic.related.iter().any(|related| {
                related.location.source == metadata_source.version
                    && related.location.span == Span { start: 0, end: 2 }
            })
            && diagnostic
                .identity
                .inputs()
                .contains(&metadata_source.version)
    }));

    assert!(
        result
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Analysis)
    );

    replace(&project, &metadata, &valid_metadata)?;

    let restored = complete(project.lint(
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

    assert!(
        restored.availability.iter().any(|availability| {
            availability.rule == RuleIdentity::RobloxIncorrectColor3NewBounds
                && availability.status == Availability::Available
        }),
        "{restored:#?}",
    );

    assert!(rule(
        &restored,
        RuleIdentity::RobloxIncorrectColor3NewBounds
    ));

    assert!(rule(&restored, RuleIdentity::DivideByZero));

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

    let project = project();

    let initial = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(shared.clone())],
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let source = initial
        .snapshot
        .source_at(&shared)
        .expect("shared source")
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

    let limits = options();

    let result = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits.clone(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        {
            let report = complete(project.lint(
                LintRequest {
                    invocation: Invocation {
                        inputs: entries.to_vec(),
                        ..Invocation::default()
                    },
                    ..LintRequest::default()
                },
                limits.clone(),
            ))
            .map_err(io::Error::from)?;
            (report.findings, report.sources, report.completion)
        },
        (
            result.findings.clone(),
            result.sources.clone(),
            result.completion.clone()
        )
    );

    assert_eq!(result.modules.len(), 2);

    let findings = result
        .module_findings
        .iter()
        .filter(|finding| finding.diagnostic.code == RuleIdentity::DivideByZero.name())
        .collect::<Vec<_>>();

    assert_eq!(findings.len(), 2);
    assert_ne!(findings[0].module, findings[1].module);
    let revision = findings[0].diagnostic.primary.source.revision;

    replace(
        &project,
        &shared,
        include_str!("../fixtures/linting/source/replacement.luau"),
    )?;

    let updated = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits.clone(),
    ))
    .map_err(io::Error::from)?;

    assert!(!rule(&updated, RuleIdentity::DivideByZero));

    assert_ne!(
        project
            .snapshot()
            .source_at(&shared)
            .expect("source")
            .version
            .revision,
        revision
    );

    Ok(())
}

#[test]
fn syntax_jobs_preserve_order_and_coordinator_progress() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/shared/.luaurc")))?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/linting/shared/instar.toml"),
    )?;

    directory.file("dependency.luau", "return 1")?;

    directory.file("child/instar.toml",
        "linting = { rules = { divide_by_zero = { enabled = false, severity = 'Warning', settings = { rule = 'divide_by_zero', parameters = {} } } } }")?;

    let mut entries = Vec::new();

    for index in 0..8 {
        let prefix = "do local value = 1 end\n".repeat((8 - index) * 16);

        let source = format!(
            "{prefix}local path='./dependency'\nlocal dependency=require(path)\nreturn 1/0"
        );

        entries.push(Input::Path(
            directory.file(&format!("entry{index}.luau"), &source)?,
        ));
    }

    entries.push(Input::Path(
        directory.file("child/entry.luau", "return 1/0")?,
    ));

    let project = project();
    let mut limits = options();

    let original = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits.clone(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(original.completion, Completion::Complete);
    assert_eq!(original.sources.len(), 10);
    assert!(rule(&original, RuleIdentity::RestrictedModulePaths));

    assert_eq!(
        original
            .findings
            .iter()
            .filter(|diagnostic| diagnostic.code == RuleIdentity::DivideByZero.name())
            .count(),
        8
    );

    let progress = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&progress);
    let coordinator = Arc::new(Mutex::new(None));

    limits.observer = Some(Arc::new(move |event| {
        if let EventPayload::Progress(event) = event.payload
            && event.phase == "Syntax linting"
        {
            assert_eq!(
                *coordinator
                    .lock()
                    .expect("syntax coordinator")
                    .get_or_insert_with(|| thread::current().id()),
                thread::current().id()
            );

            recorded
                .lock()
                .expect("syntax progress")
                .push((event.completed, event.total));
        }
    }));

    assert_eq!(
        {
            let report = complete(project.lint(
                LintRequest {
                    invocation: Invocation {
                        inputs: entries.to_vec(),
                        ..Invocation::default()
                    },
                    ..LintRequest::default()
                },
                limits.clone(),
            ))
            .map_err(io::Error::from)?;
            (report.findings, report.sources, report.completion)
        },
        (
            original.findings.clone(),
            original.sources.clone(),
            original.completion.clone()
        )
    );

    let progress = progress.lock().expect("syntax progress");
    assert_eq!(progress.first(), Some(&(0, Some(10))));
    assert_eq!(progress.last(), Some(&(10, Some(10))));
    assert!(progress.windows(2).all(|events| events[0].0 <= events[1].0));

    Ok(())
}

#[test]
fn syntax_jobs_preserve_native_syntax_deduplication() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    directory.file("instar.toml", "linting.selected = ['LocalUnused']")?;

    let first = directory.file("first.luau", "local unused=1\nreturn )")?;

    let entries = [
        Input::Path(first.clone()),
        Input::Path(directory.file("second.luau", "return (")?),
    ];

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

    let limits = options();

    let original = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits.clone(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        {
            let report = complete(project.lint(
                LintRequest {
                    invocation: Invocation {
                        inputs: entries.to_vec(),
                        ..Invocation::default()
                    },
                    ..LintRequest::default()
                },
                limits.clone(),
            ))
            .map_err(io::Error::from)?;
            (report.findings, report.sources, report.completion)
        },
        (
            original.findings.clone(),
            original.sources.clone(),
            original.completion.clone()
        )
    );

    let version = checked
        .snapshot
        .source_at(&first)
        .expect("first source")
        .version;

    let primary = Location {
        source: version,
        span: Span { start: 6, end: 12 },
    };

    let mut expected = vec![Diagnostic {
        identity: FindingIdentity::new("luau", vec![version], "7", primary),
        code: "LocalUnused".to_owned(),
        category: DiagnosticCategory::Lint,
        severity: Severity::Warning,
        primary,
        message: "Variable 'unused' is never used; prefix with '_' to silence".to_owned(),
        related: Vec::new(),
        tags: vec![DiagnosticTag::Unnecessary],
    }];

    assert_eq!(checked.sources.len(), 2);

    assert_eq!(
        original.completion,
        Completion::Incomplete(vec![Reason::UnavailableSemantics]),
    );

    for source in &checked.sources {
        let primary = Location {
            source: *source,
            span: Span { start: 0, end: 0 },
        };

        let message = "LocalUnused has incomplete coverage: UnavailableSemantics";

        expected.push(Diagnostic {
            identity: FindingIdentity::new(
                "project",
                vec![*source],
                format!("lint.prerequisite:{message}"),
                primary,
            ),
            code: "lint.prerequisite".to_owned(),
            category: DiagnosticCategory::Analysis,
            severity: Severity::Error,
            primary,
            message: message.to_owned(),
            related: Vec::new(),
            tags: Vec::new(),
        });
    }

    expected.extend(checked.findings.into_iter());

    expected.sort_by(|left, right| {
        left.primary
            .source
            .cmp(&right.primary.source)
            .then_with(|| left.primary.span.start.cmp(&right.primary.span.start))
            .then_with(|| left.code.cmp(&right.code))
    });

    assert_eq!(original.findings, expected);

    Ok(())
}

#[test]
fn syntax_jobs_stop_accepting_results_after_cancellation() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/shared/.luaurc")))?;
    let mut entries = Vec::new();

    for index in 0..8 {
        entries.push(Input::Path(
            directory.file(&format!("entry{index}.luau"), "return 1/0")?,
        ));
    }

    let project = project();

    for completed in [0, 1] {
        let mut limits = options();
        let cancellation = limits.cancellation.clone();

        limits.observer = Some(Arc::new(move |event| {
            if let EventPayload::Progress(event) = event.payload
                && event.phase == "Syntax linting"
                && event.completed == completed
            {
                cancellation.cancel();
            }
        }));

        let interrupted = complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..LintRequest::default()
            },
            limits,
        ));

        match interrupted {
            Ok(report) => {
                assert_eq!(report.completion, Completion::Cancelled);
                assert_eq!(report.modules.len(), 8);
                assert_eq!(report.findings.len(), completed);
            }

            Err(failure) => {
                assert_eq!(failure.completion, Completion::Cancelled);
                assert_eq!(failure.diagnostics.len(), completed);
                let snapshot = project.snapshot();

                let sources = entries
                    .iter()
                    .map(|input| {
                        let Input::Path(path) = input else {
                            panic!("fixture path");
                        };

                        snapshot.source_at(path).expect("prepared source").version
                    })
                    .collect::<Vec<SourceVersion>>();

                assert_eq!(sources.len(), 8);

                assert!(
                    failure
                        .diagnostics
                        .iter()
                        .all(|finding| sources.contains(&finding.primary.source))
                );

                assert!(
                    failure
                        .completed_sources
                        .iter()
                        .all(|outcome| sources.contains(&outcome.source))
                );
            }
        }

        let retry = options();

        let recovered = complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..LintRequest::default()
            },
            retry.clone(),
        ))
        .map_err(io::Error::from)?;

        assert_eq!(recovered.completion, Completion::Complete);
        assert_eq!(recovered.findings.len(), 8);
    }

    Ok(())
}

#[test]
fn syntax_jobs_keep_the_elapsed_operation_budget() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/shared/.luaurc")))?;

    let entries = [
        Input::Path(directory.file("first.luau", "return 1/0")?),
        Input::Path(directory.file("second.luau", "return 1/0")?),
    ];

    let project = project();

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

    let mut limits = Options::new(Some(Instant::now() + Duration::from_secs(1)));
    let reached = Arc::new(Mutex::new(false));
    let observed = Arc::clone(&reached);
    let timeout = limits.remaining().expect("deadline");

    limits.observer = Some(Arc::new(move |event| {
        if let EventPayload::Progress(event) = event.payload
            && event.phase == "Syntax linting"
            && event.completed == 0
        {
            *observed.lock().expect("syntax deadline") = true;
            thread::sleep(timeout);
        }
    }));

    let interrupted = complete(project.lint(
        LintRequest {
            invocation: Invocation {
                inputs: entries.to_vec(),
                ..Invocation::default()
            },
            ..LintRequest::default()
        },
        limits,
    ));

    assert!(*reached.lock().expect("syntax deadline"));

    match interrupted {
        Ok(report) => {
            assert_eq!(report.completion, Completion::TimedOut);
            assert_eq!(report.findings, Vec::new());
        }

        Err(failure) => {
            assert_eq!(failure.completion, Completion::TimedOut);
            assert_eq!(failure.diagnostics, Vec::new());
        }
    }

    Ok(())
}

#[test]
fn cancellation_and_timeout_are_explicit() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/native/strict.json")))?;

    let entry = directory.file("entry.luau", "return 1")?;
    let project = project();
    let entries = [Input::Path(entry)];

    assert_eq!(
        complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..LintRequest::default()
            },
            Options::new(Some(Instant::now()))
        ))
        .expect_err("expired invocation")
        .completion,
        Completion::TimedOut
    );

    let cancelled = options();
    cancelled.cancellation.cancel();

    assert_eq!(
        complete(project.lint(
            LintRequest {
                invocation: Invocation {
                    inputs: entries.to_vec(),
                    ..Invocation::default()
                },
                ..LintRequest::default()
            },
            cancelled
        ))
        .expect_err("cancelled invocation")
        .completion,
        Completion::Cancelled
    );

    Ok(())
}

#[test]
fn native_analysis_identity_survives_lint_conversion() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/linting/conversion/strict.json"
    )))?;

    directory.file("instar.toml", "linting.selected = [\"TableOperations\"]")?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/linting/conversion/entry.luau"),
    )?;

    let project = project();
    let entries = [Input::Path(entry.clone())];

    for source in [
        include_str!("../fixtures/linting/conversion/entry.luau"),
        include_str!("../fixtures/linting/conversion/input.luau"),
        "local =",
    ] {
        replace(&project, &entry, &source.to_owned())?;

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

        assert_eq!(checked.completion, Completion::Complete);

        assert_eq!(
            linted.completion,
            Completion::Incomplete(vec![Reason::UnavailableSemantics]),
        );

        assert_eq!(linted.availability.len(), 1);
        assert_eq!(linted.availability[0].rule, RuleIdentity::TableOperations);

        assert_eq!(
            linted.availability[0].status,
            Availability::Partial(Reason::UnavailableSemantics),
        );

        assert_eq!(linted.sources, checked.sources);

        let version = project
            .snapshot()
            .source_at(&entry)
            .expect("source")
            .version;

        assert_eq!(checked.sources, vec![version]);
        assert_ne!(checked.findings, Vec::new());

        assert!(
            checked
                .findings
                .iter()
                .all(|diagnostic| (diagnostic.identity.producer() == "luau"
                    && diagnostic.code.parse::<u32>().is_ok())
                    || (diagnostic.identity.producer() == "project"
                        && diagnostic.code == "resolution.unresolved"
                        && diagnostic.category == DiagnosticCategory::Resolution))
        );

        let message = "TableOperations has incomplete coverage: UnavailableSemantics";

        let primary = Location {
            source: version,
            span: Span { start: 0, end: 0 },
        };

        let prerequisite = Diagnostic {
            identity: FindingIdentity::new(
                "project",
                vec![version],
                format!("lint.prerequisite:{message}"),
                primary,
            ),
            code: "lint.prerequisite".to_owned(),
            category: DiagnosticCategory::Analysis,
            severity: Severity::Error,
            message: message.to_owned(),
            primary,
            related: Vec::new(),
            tags: Vec::new(),
        };

        assert_eq!(
            linted
                .findings
                .iter()
                .filter(|diagnostic| { diagnostic.code == "lint.prerequisite" })
                .cloned()
                .collect::<Vec<_>>(),
            vec![prerequisite],
        );

        assert_eq!(
            linted
                .findings
                .into_iter()
                .filter(|diagnostic| { diagnostic.code != "lint.prerequisite" })
                .collect::<Vec<_>>(),
            checked
                .findings
                .into_iter()
                .filter(|diagnostic| { diagnostic.category != DiagnosticCategory::Type })
                .collect::<Vec<_>>(),
        );
    }

    replace(&project, &entry, "return 1")?;

    replace(
        &project,
        &directory.path().join("instar.toml"),
        include_str!("../fixtures/linting/missing/instar.toml"),
    )?;

    for declaration in [
        include_str!("../fixtures/linting/missing/invalid.luau"),
        include_str!("../fixtures/linting/conversion/definition.luau"),
    ] {
        replace(
            &project,
            &directory.path().join("globals.luau"),
            &declaration.to_owned(),
        )?;

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
            Completion::Incomplete(vec![Reason::UnavailableEnvironment])
        );

        assert_eq!(linted.completion, checked.completion);
        assert_ne!(checked.findings, Vec::new());

        let native = checked
            .findings
            .iter()
            .filter(|diagnostic| diagnostic.identity.producer() == "luau")
            .collect::<Vec<_>>();

        assert!(!native.is_empty());

        assert!(
            native
                .iter()
                .all(|diagnostic| diagnostic.code.parse::<u32>().is_ok())
        );

        let environment = checked
            .findings
            .iter()
            .filter(|diagnostic| {
                diagnostic.identity.producer() == "project"
                    && diagnostic.code == "environment.unavailable"
                    && diagnostic.category == DiagnosticCategory::Analysis
                    && diagnostic.severity == Severity::Error
            })
            .collect::<Vec<_>>();

        assert_eq!(environment.len(), 1);
        assert_eq!(checked.findings.len(), native.len() + environment.len());

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
            linted
                .findings
                .into_iter()
                .filter(|diagnostic| { diagnostic.identity.producer() != "lint" })
                .collect::<Vec<_>>(),
            checked.findings,
        );
    }

    Ok(())
}

#[test]
fn runtime_member_requires_preserve_lint_rules_and_static_failures() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!("../fixtures/linting/shared/.luaurc")))?;

    directory.file(
        "instar.toml",
        "environment.roblox = true\nlinting.selected = ['divide_by_zero']\n",
    )?;

    directory.file(
        "sourcemap.json",
        include_str!("../fixtures/lifecycle/reference/sourcemap.json"),
    )?;

    let source = "require(script.Parent.Reference.Value)\nrequire(script.Parent.Model.PrimaryPart)\nreturn 1/0";
    let entry = directory.file("caller.luau", source)?;
    let entries = [Input::Path(entry.clone())];
    let project = project();

    for configuration in [
        include_str!("../fixtures/linting/shared/.luaurc"),
        include_str!("../fixtures/linting/conversion/strict.json"),
    ] {
        replace(
            &project,
            &directory.path().join(".luaurc"),
            &configuration.to_owned(),
        )?;

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

        assert_eq!(linted.completion, Completion::Complete);
        assert!(rule(&linted, RuleIdentity::DivideByZero));

        assert!(!linted.findings.iter().any(|diagnostic| matches!(
            diagnostic.category,
            DiagnosticCategory::Resolution | DiagnosticCategory::Type
        )));

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

        assert_eq!(checked.completion, Completion::Complete);

        assert_eq!(
            checked
                .imports
                .iter()
                .filter(|site| matches!(site.outcome, ImportOutcome::Nonconstant))
                .count(),
            2
        );

        assert!(
            !checked
                .findings
                .iter()
                .any(|finding| finding.category == DiagnosticCategory::Resolution)
        );

        assert_eq!(
            {
                let report = complete(project.lint(
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
                (report.findings, report.sources, report.completion)
            },
            (
                linted.findings.clone(),
                linted.sources.clone(),
                linted.completion.clone()
            )
        );
    }

    replace(
        &project,
        &directory.path().join("instar.toml"),
        "environment.roblox = true\nlinting.selected = ['divide_by_zero', 'non_const_require']\n",
    )?;

    let configured = complete(project.lint(
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
        configured
            .findings
            .iter()
            .filter(|diagnostic| diagnostic.code == RuleIdentity::NonConstRequire.name())
            .count(),
        2
    );

    replace(&project, &entry, &format!("require('./missing')\n{source}"))?;

    let missing = complete(project.lint(
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

    assert_eq!(missing.completion, Completion::Complete);
    assert!(rule(&missing, RuleIdentity::DivideByZero));
    assert!(rule(&missing, RuleIdentity::NonConstRequire));

    let failures = missing
        .findings
        .iter()
        .filter(|diagnostic| diagnostic.category == DiagnosticCategory::Resolution)
        .collect::<Vec<_>>();

    assert_eq!(failures.len(), 1);

    assert_eq!(
        &project.snapshot().source_at(&entry).expect("source").text
            [failures[0].primary.span.start..failures[0].primary.span.end],
        "'./missing'"
    );

    Ok(())
}

#[test]
fn source_type_errors_stay_with_check_while_lint_findings_remain() -> io::Result<()> {
    let directory = Directory::new(Some(include_str!(
        "../fixtures/linting/conversion/strict.json"
    )))?;

    directory.file("instar.toml", "linting.selected = [\"divide_by_zero\"]")?;

    let entry = directory.file(
        "entry.luau",
        "local number: number = 'bad'\nreturn number/0",
    )?;

    let entries = [Input::Path(entry)];
    let project = project();

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

    assert_eq!(linted.completion, Completion::Complete);
    assert!(rule(&linted, RuleIdentity::DivideByZero));

    assert!(
        !linted
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Type)
    );

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

    assert_eq!(checked.completion, Completion::Complete);

    assert!(
        checked
            .findings
            .iter()
            .any(|diagnostic| diagnostic.category == DiagnosticCategory::Type)
    );

    assert_eq!(
        {
            let report = complete(project.lint(
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
            (report.findings, report.sources, report.completion)
        },
        (
            linted.findings.clone(),
            linted.sources.clone(),
            linted.completion.clone()
        )
    );

    Ok(())
}
