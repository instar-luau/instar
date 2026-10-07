//! Synthetic configuration discovery and inheritance fixtures.
use std::{fs, io, path::Path, time::Duration};

use instar_core::project::Project;
use serde_json::{Value, json};

use crate::support::{Directory, copy, normalize};

#[test]
fn configuration_projects() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");

    let mut cases = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;

    cases.sort();

    for case in cases {
        let directory = Directory::new(None)?;
        copy(&case.join("input"), &directory.path, None)?;

        let relative = match fs::read_to_string(case.join("path.txt")) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };

        let mut view = Project::new(Duration::from_secs(2));
        let result = view.configuration(&directory.path.join(relative.trim()));

        if case.join("error.txt").exists() {
            let expected = fs::read_to_string(case.join("error.txt"))?;
            let error = result.err().expect("invalid fixture must fail").to_string();

            assert!(
                error.contains(expected.trim()),
                "{}: {error}",
                case.display()
            );

            assert!(
                error.contains(&directory.path.to_string_lossy().to_string()),
                "{}: error must name its origin: {error}",
                case.display()
            );
        } else {
            let project = result?;
            let mut output = json!({"configuration": project.configuration, "native": project.native.snapshot()?});

            normalize(
                &mut output,
                &directory.path.to_string_lossy().replace('\\', "/"),
            );

            let expected: serde_json::Map<String, Value> =
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
