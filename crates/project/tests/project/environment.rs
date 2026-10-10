use std::{io, path::Path};

use instar_project::{
    ChangeBatch, CheckOverrides, CheckRequest, Input, InputIdentity, InputState, Invocation,
    OperationOverrides, Project, ProjectChange, Query, QueryOutput, QueryReport, QueryRequest,
    SettingsRequest, roblox_cache_directory,
};

use instar_shared::{Completion, OperationFailure, SourceVersion};

use crate::support::{Directory, complete, options, project, replace};

fn lookup(project: &Project, entry: &Path, symbol: &str) -> Result<QueryReport, OperationFailure> {
    let snapshot = project.snapshot();
    let source = snapshot.source_at(entry).expect("query source").version;

    complete(project.query(
        QueryRequest {
            snapshot,
            operation: Query::Documentation {
                source,
                symbol: symbol.to_owned(),
            },
        },
        options(),
    ))
}

#[test]
fn configuration_rejects_reserved_namespaces_and_invalid_flags() -> io::Result<()> {
    for source in [
        include_str!("../fixtures/environment/configuration/luau.toml"),
        include_str!("../fixtures/environment/configuration/roblox.toml"),
        include_str!("../fixtures/environment/configuration/prefix.toml"),
        include_str!("../fixtures/environment/configuration/duplicate.toml"),
        include_str!("../fixtures/environment/configuration/missing.toml"),
        include_str!("../fixtures/environment/configuration/boolean.toml"),
        include_str!("../fixtures/environment/configuration/string.toml"),
        include_str!("../fixtures/environment/configuration/overflow.toml"),
    ] {
        let directory = Directory::new(None)?;
        let entry = directory.file("entry.luau", "return 1")?;
        directory.file("instar.toml", source)?;
        let project = Project::new(vec![directory.path().to_path_buf()]);

        assert!(
            complete(project.settings(
                SettingsRequest {
                    snapshot: None,
                    invocation: Invocation {
                        inputs: vec![Input::Path(entry)],
                        ..Invocation::default()
                    },
                    overrides: OperationOverrides::Check(CheckOverrides::default()),
                },
                options()
            ))
            .is_err(),
            "{source}"
        );
    }

    Ok(())
}

#[test]
fn documentation_lookup_tracks_overlays_and_rejects_invalid_entries() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let configuration = directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/documentation/instar.toml"),
    )?;

    let entry = directory.file("entry.luau", "return 1")?;

    let documentation = directory.file(
        "docs.json",
        include_str!("../fixtures/environment/documentation/docs.json"),
    )?;

    let project = project();
    replace(&project, &entry, "return 1")?;
    let first = lookup(&project, &entry, "@custom/global/value").map_err(io::Error::from)?;

    let QueryOutput::Documentation(Some(first)) = first.output else {
        panic!("symbol");
    };

    assert_eq!(first.documentation.as_deref(), Some("first"));

    replace(
        &project,
        &documentation,
        include_str!("../fixtures/environment/documentation/input.json"),
    )?;

    let updated = lookup(&project, &entry, "@custom/global/value").map_err(io::Error::from)?;

    let QueryOutput::Documentation(Some(updated)) = updated.output else {
        panic!("symbol");
    };

    assert_eq!(updated.documentation.as_deref(), Some("updated"));

    for text in [
        include_str!("../fixtures/environment/documentation/replacement.json"),
        include_str!("../fixtures/environment/documentation/invalid.json"),
        include_str!("../fixtures/environment/documentation/alternate.json"),
        include_str!("../fixtures/environment/documentation/final.json"),
    ] {
        replace(&project, &documentation, text)?;

        assert!(
            lookup(&project, &entry, "@custom/global/value").is_err(),
            "{text}"
        );
    }

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/environment/documentation/input.toml"),
    )?;

    let removed = lookup(&project, &entry, "@custom/global/value").map_err(io::Error::from)?;
    assert!(matches!(removed.output, QueryOutput::Documentation(None)));

    Ok(())
}

#[test]
fn project_flags_follow_configuration_changes() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let configuration = directory.file("instar.toml", "")?;
    let source = format!("return {}1{}", "(".repeat(40), ")".repeat(40));
    let entry = directory.file("entry.luau", &source)?;
    let project = project();

    let request = CheckRequest {
        invocation: Invocation {
            inputs: vec![Input::Path(entry)],
            ..Invocation::default()
        },
        ..CheckRequest::default()
    };

    assert_eq!(
        complete(project.check(request.clone(), options()))
            .map_err(io::Error::from)?
            .findings,
        Vec::new()
    );

    replace(
        &project,
        &configuration,
        include_str!("../fixtures/environment/project/input.toml"),
    )?;

    assert_ne!(
        complete(project.check(request.clone(), options()))
            .map_err(io::Error::from)?
            .findings,
        Vec::new()
    );

    replace(&project, &configuration, "")?;

    assert_eq!(
        complete(project.check(request, options()))
            .map_err(io::Error::from)?
            .findings,
        Vec::new()
    );

    Ok(())
}

#[test]
fn parent_environment_paths_invalidate_consumers() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "nested/instar.toml",
        include_str!("../fixtures/environment/parent/nested/instar.toml"),
    )?;

    let entry = directory.file(
        "nested/entry.luau",
        include_str!("../fixtures/environment/parent/nested/entry.luau"),
    )?;

    let definitions = directory.file(
        "types.luau",
        include_str!("../fixtures/environment/parent/types.luau"),
    )?;

    let documentation = directory.file("docs.json", "{}")?;
    let project = project();

    replace(
        &project,
        &entry,
        include_str!("../fixtures/environment/parent/nested/entry.luau"),
    )?;

    for path in [definitions, documentation] {
        let report = complete(project.check(
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

        let consulted = report
            .inputs
            .iter()
            .find(|input| input.identity == InputIdentity::Path(path.clone()))
            .expect("consulted environment path")
            .clone();

        assert!(project.current(&report.inputs));
        let mut changed = consulted.clone();
        changed.revision += 1;
        changed.state = InputState::Stale;

        project.change(ChangeBatch {
            changes: vec![ProjectChange::Input {
                expected: Some(consulted),
                current: changed,
            }],
        })?;

        assert!(!project.current(&report.inputs));
    }

    Ok(())
}

#[test]
fn cancelled_preparation_and_documentation_do_not_load_sources() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let entry = directory.file("entry.luau", "return 1")?;
    let project = project();
    let original = project.snapshot();
    let limits = options();
    limits.cancellation.cancel();

    let failure = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path(entry.clone())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        limits.clone(),
    ))
    .expect_err("cancelled preparation");

    assert_eq!(failure.completion, Completion::Cancelled);
    assert_eq!(io::Error::from(failure).kind(), io::ErrorKind::Interrupted);
    assert_eq!(project.snapshot().identity, original.identity);

    assert_eq!(
        project.snapshot().sources().count(),
        original.sources().count()
    );

    let identity = project.source_identity();

    let failure = complete(project.query(
        QueryRequest {
            snapshot: original,
            operation: Query::Documentation {
                source: SourceVersion {
                    identity,
                    revision: 1,
                },
                symbol: "@luau/global/print".to_owned(),
            },
        },
        limits,
    ))
    .expect_err("cancelled documentation");

    assert_eq!(failure.completion, Completion::Cancelled);
    assert_eq!(io::Error::from(failure).kind(), io::ErrorKind::Interrupted);

    Ok(())
}

#[test]
fn builtin_documentation_loads_without_roblox_assets() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/builtin/instar.toml"),
    )?;

    let entry = directory.file(
        "entry.luau",
        include_str!("../fixtures/environment/builtin/entry.luau"),
    )?;

    let project = project();

    replace(
        &project,
        &entry,
        include_str!("../fixtures/environment/builtin/entry.luau"),
    )?;

    replace(
        &project,
        &roblox_cache_directory()?.join("metadata.json"),
        "invalid Roblox metadata",
    )?;

    let documentation = lookup(&project, &entry, "@luau/global/print").map_err(io::Error::from)?;

    let QueryOutput::Documentation(Some(documentation)) = documentation.output else {
        panic!("builtin documentation");
    };

    assert_eq!(
        documentation.documentation.as_deref(),
        Some("Prints values.")
    );

    assert!(matches!(
        lookup(&project, &entry, "@roblox/global/game")
            .map_err(io::Error::from)?
            .output,
        QueryOutput::Documentation(None)
    ));

    Ok(())
}

#[test]
fn deserialization_enforces_documentation_semantics() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/documentation/instar.toml"),
    )?;

    let entry = directory.file("entry.luau", "return 1")?;
    let documentation = directory.path().join("docs.json");
    let project = project();
    replace(&project, &entry, "return 1")?;

    for source in [
        include_str!("../fixtures/environment/documentation/alternate.json"),
        include_str!("../fixtures/environment/deserialization/input.json"),
        include_str!("../fixtures/environment/deserialization/replacement.json"),
        include_str!("../fixtures/environment/deserialization/invalid.json"),
        include_str!("../fixtures/environment/deserialization/alternate.json"),
    ] {
        replace(&project, &documentation, source)?;

        assert!(
            lookup(&project, &entry, "@custom/global/value").is_err(),
            "{source}"
        );
    }

    Ok(())
}
