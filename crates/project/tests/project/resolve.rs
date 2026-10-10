use std::{error::Error, fs, io, path::Path};

use instar_luau::ImportOutcome;

use instar_project::{
    CheckOverrides, CheckRequest, Input, Invocation, OperationOverrides, Project, SettingsRequest,
    SourceOrigin,
};

use instar_shared::{DiagnosticCategory, FailureKind};
use serde::Deserialize;
use serde_json::{Value, json};
use similar_asserts::assert_eq;

use crate::support::{Directory, complete, copy, normalize, options, replace};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    source: String,
    request: String,
}

#[test]
fn require_fixtures() -> Result<(), Box<dyn Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolve");

    let mut cases = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;

    cases.sort();

    for case in cases {
        let directory = Directory::new(None)?;
        let root = directory.path().to_string_lossy().replace('\\', "/");
        copy(&case.join("input"), directory.path(), Some(&root))?;
        let project = Project::new(vec![directory.path().to_path_buf()]);

        let requests: Vec<Request> =
            serde_json::from_str(&fs::read_to_string(case.join("requests.json"))?)?;

        let expected: Vec<Value> =
            serde_json::from_str(&fs::read_to_string(case.join("expected.json"))?)?;

        assert!(
            !requests.is_empty(),
            "{}: fixture must have requests",
            case.display()
        );

        assert_eq!(
            requests.len(),
            expected.len(),
            "{}: every request needs an oracle",
            case.display()
        );

        for (request, expected) in requests.iter().zip(expected) {
            let source = directory.path().join(&request.source);
            let argument = request.request.replace("$ROOT", &root);

            if source.file_name().is_none_or(|name| name != "instar.luau") {
                replace(
                    &project,
                    &source,
                    &format!("return require({})", json!(argument)),
                )?;
            }

            let result = complete(project.check(
                CheckRequest {
                    invocation: Invocation {
                        inputs: vec![Input::Path(source.clone())],
                        ..Invocation::default()
                    },
                    ..CheckRequest::default()
                },
                options(),
            ));

            let outcome = match result {
                Ok(report) => {
                    let caller = report
                        .snapshot
                        .source_at(&source)
                        .expect("caller")
                        .version
                        .identity;

                    let site = report
                        .imports
                        .iter()
                        .find(|site| site.importer.source == caller)
                        .expect("every fixture has one require request");

                    if let ImportOutcome::Module(module) = site.outcome {
                        let source = report
                            .snapshot
                            .origin(module.source)
                            .and_then(SourceOrigin::path)
                            .expect("resolved source");

                        let path = if matches!(
                            source.file_name().and_then(|name| name.to_str()),
                            Some("init.lua" | "init.luau")
                        ) {
                            source.parent().expect("module parent").to_path_buf()
                        } else {
                            source.with_extension("")
                        };

                        Ok(json!({"module": {"path": path, "source": source}}))
                    } else {
                        Err(site
                            .failure
                            .as_ref()
                            .expect("static require must resolve or fail")
                            .message
                            .clone())
                    }
                }

                Err(failure) => match failure.kind {
                    FailureKind::InvalidInvocation => Err(failure.message),

                    FailureKind::Operational => {
                        let diagnostics = failure
                            .diagnostics
                            .iter()
                            .filter(|finding| finding.category == DiagnosticCategory::Configuration)
                            .collect::<Vec<_>>();

                        let diagnostic = diagnostics.first().expect("configuration failure cause");

                        assert!(
                            diagnostics
                                .iter()
                                .all(|finding| finding.message == diagnostic.message)
                        );

                        assert!(diagnostics.iter().all(|finding| {
                            project
                                .snapshot()
                                .origin(finding.primary.source.identity)
                                .and_then(SourceOrigin::path)
                                .is_some()
                        }));

                        Err(diagnostic.message.clone())
                    }

                    FailureKind::Busy => panic!("fixture admission: {failure}"),
                },
            };

            let mut output = outcome.unwrap_or_else(|message| json!({"error": format!("{}: require {:?}: {message}", source.display(), argument)}));
            normalize(&root, &mut output);

            assert_eq!(
                output,
                expected,
                "{}: {} -> {:?}",
                case.display(),
                request.source,
                request.request
            );
        }
    }

    Ok(())
}

#[test]
fn relative_callers_are_rejected() {
    let project = Project::new(Vec::new());

    let failure = complete(project.check(
        CheckRequest {
            invocation: Invocation {
                inputs: vec![Input::Path("caller.luau".into())],
                ..Invocation::default()
            },
            ..CheckRequest::default()
        },
        options(),
    ))
    .expect_err("a relative caller would depend on cwd");

    assert_eq!(failure.message, "input path must be absolute");
}

#[test]
fn native_alias_rejections_are_transactional() -> io::Result<()> {
    for (filename, source, message) in [
        (
            ".luaurc",
            include_str!("../fixtures/resolve/reserved/input/.luaurc"),
            "alias @self is reserved",
        ),
        (
            "instar.luau",
            include_str!("../fixtures/resolve/self/input/instar.luau"),
            "alias @self is reserved",
        ),
        (
            ".luaurc",
            include_str!("../fixtures/resolve/prefixed/input/.luaurc"),
            "invalid module alias name @named",
        ),
        (
            "instar.luau",
            include_str!("../fixtures/resolve/prefix/input/instar.luau"),
            "invalid module alias name @named",
        ),
    ] {
        let directory = Directory::new(None)?;

        let valid = if filename == ".luaurc" {
            r#"{"aliases":{"retained":"./target"}}"#
        } else {
            "return { resolution = { aliases = { retained = './target' } } }"
        };

        let configuration = directory.file(filename, valid)?;
        let entry = directory.file("entry.luau", "return 1")?;
        let project = Project::new(vec![directory.path().to_path_buf()]);

        let invocation = Invocation {
            inputs: vec![Input::Path(entry)],
            ..Invocation::default()
        };

        let previous = complete(project.settings(
            SettingsRequest {
                snapshot: None,
                invocation: invocation.clone(),
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options(),
        ))
        .map_err(io::Error::from)?;

        replace(&project, &configuration, source)?;

        let failure = complete(project.settings(
            SettingsRequest {
                snapshot: None,
                invocation: invocation.clone(),
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options(),
        ))
        .expect_err("invalid alias layer must fail");

        let diagnostic = failure
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.category == DiagnosticCategory::Configuration)
            .expect("invalid alias cause");

        assert!(
            diagnostic.message.contains(message),
            "{}: {}",
            filename,
            diagnostic.message
        );

        assert_eq!(
            project
                .snapshot()
                .origin(diagnostic.primary.source.identity)
                .and_then(SourceOrigin::path),
            Some(configuration.as_path())
        );

        let retained = complete(project.settings(
            SettingsRequest {
                snapshot: Some(previous.snapshot),
                invocation,
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options(),
        ))
        .map_err(io::Error::from)?;

        assert_eq!(
            serde_json::to_value(retained.settings[0].settings.clone())
                .map_err(io::Error::other)?,
            serde_json::to_value(previous.settings[0].settings.clone())
                .map_err(io::Error::other)?
        );
    }

    Ok(())
}
