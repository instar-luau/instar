use std::{fmt::Write, io, path::Path};

use instar_luau::ModuleIdentity;

use instar_project::{
    ChangeBatch, CheckOverrides, CheckRequest, Input, InputIdentity, InputState, Invocation,
    OperationOverrides, PositionQuery, PositionQueryKind, Project, ProjectChange, Query,
    QueryOutput, QueryReport, QueryRequest, SettingsRequest, SourceChange,
    documentation_cache_directory, roblox_cache_directory,
};

use instar_shared::{
    Availability, Completion, FailureKind, Knowledge, OperationFailure, SourceVersion,
};

use serde_json::json;
use sha2::{Digest, Sha256};

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
fn builtin_semantics_work_without_optional_documentation() -> io::Result<()> {
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

    let metadata_path = documentation_cache_directory()?.join("metadata.json");

    let metadata = project
        .snapshot()
        .source_at(&metadata_path)
        .expect("builtin metadata")
        .version;

    project.change(ChangeBatch {
        changes: vec![ProjectChange::Source(SourceChange::Remove {
            expected: metadata,
        })],
    })?;

    for kind in [PositionQueryKind::Hover, PositionQueryKind::Signatures] {
        let snapshot = project.snapshot();
        let source = snapshot.source_at(&entry).expect("query source").version;

        let report = complete(project.query(
            QueryRequest {
                operation: Query::Inspect(PositionQuery {
                    module: ModuleIdentity {
                        context: snapshot.context(source.identity),
                        source: source.identity,
                    },
                    source,
                    offset: if matches!(kind, PositionQueryKind::Hover) {
                        1
                    } else {
                        7
                    },
                    kind,
                }),
                snapshot,
            },
            options(),
        ))
        .map_err(io::Error::from)?;

        assert_eq!(report.completion, Completion::Complete);
        assert_eq!(report.availability, Availability::Available);

        let QueryOutput::Inspection(inspection) = report.output else {
            panic!("builtin inspection");
        };

        let occurrence = inspection.occurrence.expect("builtin occurrence");

        if matches!(kind, PositionQueryKind::Hover) {
            assert!(matches!(occurrence.type_information, Knowledge::Known(_)));
        } else {
            assert!(
                matches!(occurrence.signatures, Knowledge::Known(signatures) if !signatures.is_empty())
            );
        }

        let documentation = complete(project.query(
            QueryRequest {
                snapshot: report.snapshot,
                operation: Query::Documentation {
                    source,
                    symbol: "@luau/global/print".to_owned(),
                },
            },
            options(),
        ))
        .map_err(io::Error::from)?;

        assert_eq!(documentation.completion, Completion::Complete);

        assert!(matches!(
            documentation.output,
            QueryOutput::Documentation(None)
        ));

        let missing = documentation
            .inputs
            .iter()
            .find(|input| input.identity == InputIdentity::Path(metadata_path.clone()))
            .expect("missing metadata guard");

        assert_eq!(missing.state, InputState::Missing);

        assert_eq!(
            documentation.snapshot.input_versions.get(&missing.identity),
            Some(missing)
        );
    }

    Ok(())
}

#[test]
fn installed_builtin_documentation_rejects_corrupt_bundles() -> io::Result<()> {
    let directory = Directory::new(None)?;

    directory.file(
        "instar.toml",
        include_str!("../fixtures/environment/builtin/instar.toml"),
    )?;

    let entry = directory.file("entry.luau", "print('hello')")?;
    let project = project();
    replace(&project, &entry, "print('hello')")?;
    let cache = documentation_cache_directory()?;
    let metadata_path = cache.join("metadata.json");
    let documentation_path = cache.join("documentation.json");

    replace(&project, &metadata_path, "{")?;
    let malformed = lookup(&project, &entry, "@luau/global/print").expect_err("invalid metadata");

    assert!(
        malformed
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "documentation.metadata")
    );

    replace(
        &project,
        &metadata_path,
        include_str!("../fixtures/assets/documentation/metadata.json"),
    )?;

    replace(&project, &documentation_path, "{}")?;
    let mismatch = lookup(&project, &entry, "@luau/global/print").expect_err("digest mismatch");

    assert!(
        mismatch
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "documentation.digest")
    );

    let invalid_namespace = r#"{"@custom/global/print":{"documentation":"invalid namespace"}}"#;
    let mut digest = String::new();

    for byte in Sha256::digest(invalid_namespace.as_bytes()) {
        write!(&mut digest, "{byte:02x}").map_err(io::Error::other)?;
    }

    let metadata = json!({
        "revision": "0123456789012345678901234567890123456789",
        "documentation": digest,
    });

    replace(&project, &metadata_path, &metadata.to_string())?;
    replace(&project, &documentation_path, invalid_namespace)?;
    let invalid = lookup(&project, &entry, "@luau/global/print").expect_err("invalid namespace");

    assert!(
        invalid
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "documentation.invalid")
    );

    let payload = project
        .snapshot()
        .source_at(&documentation_path)
        .expect("builtin payload")
        .version;

    project.change(ChangeBatch {
        changes: vec![ProjectChange::Source(SourceChange::Remove {
            expected: payload,
        })],
    })?;

    let missing = lookup(&project, &entry, "@luau/global/print").expect_err("missing payload");
    assert_eq!(missing.kind, FailureKind::InvalidInvocation);
    assert!(missing.message.contains("documentation.json"));

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
