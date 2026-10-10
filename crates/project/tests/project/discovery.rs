//! Synthetic configuration discovery and inheritance fixtures.

use std::{collections::BTreeSet, error::Error, fs, io, path::Path, sync::Arc};

use instar_project::{
    CheckOverrides, CheckRequest, Input, Invocation, OperationOverrides, Project, SettingsRequest,
    SourceOrigin,
};

use serde_json::{Map, Value, json};

use crate::support::{Directory, complete, copy, normalize, options};

#[test]
fn configuration_projects() -> Result<(), Box<dyn Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");

    let mut cases = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;

    cases.sort();

    for case in cases {
        let directory = Directory::new(None)?;
        copy(&case.join("input"), directory.path(), None)?;

        let relative = match fs::read_to_string(case.join("path.txt")) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };

        let project = Project::new(vec![directory.path().to_path_buf()]);

        let result = complete(project.settings(
            SettingsRequest {
                snapshot: None,
                invocation: Invocation {
                    inputs: vec![Input::Stdin {
                        text: Arc::from("return 1"),
                        context: Some(directory.path().join(relative.trim()).join("entry.luau")),
                    }],
                    ..Invocation::default()
                },
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options(),
        ));

        if case.join("error.txt").exists() {
            let expected = fs::read_to_string(case.join("error.txt"))?;
            let failure = result.expect_err("invalid fixture must fail");
            let snapshot = project.snapshot();

            assert!(
                failure.diagnostics.iter().any(|diagnostic| {
                    diagnostic.message.contains(expected.trim())
                        && snapshot
                            .origin(diagnostic.primary.source.identity)
                            .and_then(SourceOrigin::path)
                            .is_some_and(|path| path.starts_with(directory.path()))
                }),
                "{}: error must retain its cause and origin: {failure:?}",
                case.display()
            );
        } else {
            let report = result?;
            assert_eq!(report.settings.len(), 1, "{}", case.display());
            let effective = &report.settings[0];
            let settings = &effective.settings;
            let common = &settings.selection;
            let mut selections = Map::new();

            for (name, policy) in [
                ("checking", &common.checking),
                ("formatting", &common.formatting),
                ("linting", &common.linting),
                ("index", &common.index),
                ("imports", &common.imports),
            ] {
                selections.insert(
                    name.to_owned(),
                    json!({
                        "include": policy.include.as_ref().or(common.include.as_ref()),
                        "exclude": policy.exclude.as_ref().or(common.exclude.as_ref()),
                    }),
                );
            }

            let origins = effective
                .origins
                .iter()
                .map(|origin| (origin.field.clone(), json!(origin.base)))
                .collect::<Map<_, _>>();

            let mut output =
                json!({ "settings": settings, "selection": selections, "origins": origins });

            normalize(
                &directory.path().to_string_lossy().replace('\\', "/"),
                &mut output,
            );

            let expected: Map<String, Value> =
                serde_json::from_str(&fs::read_to_string(case.join("expected.json"))?)?;

            for (pointer, expected) in expected {
                assert_eq!(
                    output.pointer(&pointer),
                    Some(&expected),
                    "{}: {pointer}",
                    case.display()
                );
            }
        }
    }

    Ok(())
}

#[test]
fn file_patterns_share_directory_and_wildcard_semantics() -> Result<(), Box<dyn Error>> {
    let fixture: Value = serde_json::from_str(include_str!("../fixtures/selection/cases.json"))?;
    let directory = Directory::new(None)?;
    let project = Project::new(vec![directory.path().to_path_buf()]);
    let sources = fixture["sources"].as_array().expect("sources");

    for case in fixture["cases"].as_array().expect("matching cases") {
        let configuration = format!("selection = {{ include = [{}] }}", case["pattern"]);
        directory.file("instar.toml", &configuration)?;

        let report = complete(
            project.check(
                CheckRequest {
                    invocation: Invocation {
                        inputs: sources
                            .iter()
                            .map(|source| Input::Stdin {
                                text: Arc::from("return 1"),
                                context: Some(
                                    directory.path().join(source.as_str().expect("source")),
                                ),
                            })
                            .collect(),
                        ..Invocation::default()
                    },
                    ..CheckRequest::default()
                },
                options(),
            ),
        )?;

        let selected = report
            .selection
            .iter()
            .filter(|decision| decision.selected)
            .filter_map(|decision| decision.source)
            .map(|identity| {
                report
                    .snapshot
                    .origin(identity)
                    .and_then(SourceOrigin::path)
                    .expect("selection context")
                    .to_path_buf()
            })
            .collect::<BTreeSet<_>>();

        let found = sources
            .iter()
            .filter_map(|source| {
                let source = source.as_str().expect("source");

                selected
                    .contains(&directory.path().join(source))
                    .then_some(source)
            })
            .collect::<Vec<_>>();

        assert_eq!(json!(found), case["expected"], "{case}");
    }

    for pattern in fixture["invalid"].as_array().expect("invalid patterns") {
        directory.file(
            "instar.toml",
            &format!("selection = {{ include = [{pattern}] }}"),
        )?;

        assert!(
            complete(project.settings(
                SettingsRequest {
                    snapshot: None,
                    invocation: Invocation {
                        inputs: vec![Input::Stdin {
                            text: Arc::from("return 1"),
                            context: Some(directory.path().join("entry.luau"))
                        }],
                        ..Invocation::default()
                    },
                    overrides: OperationOverrides::Check(CheckOverrides::default()),
                },
                options()
            ))
            .is_err(),
            "{pattern}"
        );
    }

    Ok(())
}
