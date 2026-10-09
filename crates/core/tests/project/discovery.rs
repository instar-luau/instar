//! Synthetic configuration discovery and inheritance fixtures.
use std::{error::Error, fs, io, path::Path, slice, time::Duration};

use instar_analysis::selection;
use instar_core::project::Project;
use serde_json::{Map, Value, json};

use crate::support::{Directory, copy, normalize};

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

        let mut view = Project::new(Duration::from_secs(2));
        let result = view.configuration(&directory.path().join(relative.trim()));

        if case.join("error.txt").exists() {
            let expected = fs::read_to_string(case.join("error.txt"))?;
            let error = result.err().expect("invalid fixture must fail").to_string();

            assert!(
                error.contains(expected.trim()),
                "{}: {error}",
                case.display()
            );

            assert!(
                error.contains(&directory.path().to_string_lossy().to_string()),
                "{}: error must name its origin: {error}",
                case.display()
            );
        } else {
            let project = result?;
            let mut output = json!({"configuration": project.configuration, "native": project.native.snapshot()?});

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

    for case in fixture["cases"].as_array().expect("matching cases") {
        let pattern =
            selection::anchor(directory.path(), case["pattern"].as_str().expect("pattern"))?;

        let mut found = Vec::new();

        for source in fixture["sources"].as_array().expect("sources") {
            let source = source.as_str().expect("source");

            if selection::matches(
                &directory.path().join(source),
                Some(slice::from_ref(&pattern)),
                false,
            )? {
                found.push(source);
            }
        }

        assert_eq!(json!(found), case["expected"], "{case}");
    }

    for pattern in fixture["invalid"].as_array().expect("invalid patterns") {
        let patterns = vec![pattern.as_str().expect("pattern").to_owned()];

        assert!(
            selection::validate(Some(&patterns), "selection").is_err(),
            "{pattern}"
        );
    }

    assert!(selection::string("@vendor/item", "@vendor/*")?);
    assert!(!selection::string("@vendor/nested/item", "@vendor/*")?);
    assert!(selection::string("@vendor/nested/item", "@vendor/**")?);
    assert!(!selection::string("./nested/item", "item")?);
    assert!(selection::string("./item", "./**/item")?);

    Ok(())
}
