//! Synthetic require resolution fixtures with independent expected outcomes.
use std::{error::Error, fs, io, path::Path, time::Duration};

use instar_bridge::Configuration;
use instar_core::project::Project;
use serde::Deserialize;
use serde_json::{Value, json};
use similar_asserts::assert_eq;

use crate::support::{Directory, copy, normalize};

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
        let mut project = Project::new(Duration::from_secs(2));

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

            let mut output = match project.resolve_source(&source, &argument).result {
                Ok(module) => json!({"module": {"path": module.path, "source": module.source}}),
                Err(error) => json!({"error": error.to_string()}),
            };

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
    let error = Project::new(Duration::from_secs(2))
        .resolve_source(Path::new("caller.luau"), "./target")
        .result
        .expect_err("a relative caller would depend on cwd");

    assert_eq!(
        error.to_string(),
        "caller.luau: require \"./target\": requiring source must be absolute"
    );
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
            ".config.luau",
            include_str!("../fixtures/resolve/self/input/.config.luau"),
            "alias @self is reserved",
        ),
        (
            ".luaurc",
            include_str!("../fixtures/resolve/prefixed/input/.luaurc"),
            "Invalid alias @named",
        ),
        (
            ".config.luau",
            include_str!("../fixtures/resolve/prefix/input/.config.luau"),
            "Invalid alias @named",
        ),
    ] {
        let mut native = Configuration::new()?;

        native.apply(
            r#"{"aliases":{"retained":"./target"}}"#,
            Path::new(".luaurc"),
            Duration::from_secs(2),
        )?;

        let previous = native.snapshot()?;

        let error = native
            .apply(source, Path::new(filename), Duration::from_secs(2))
            .expect_err("invalid alias layer must fail");

        assert_eq!(error.to_string(), format!("{filename}: {message}"));
        assert_eq!(native.snapshot()?, previous);
    }

    Ok(())
}
