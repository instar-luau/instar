//! Synthetic configuration discovery and inheritance fixtures.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use instar_core::project::Project;
use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "instar-configuration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        fs::create_dir(&path)?;

        Ok(Self(path))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove synthetic configuration fixture");
    }
}

fn copy(source: &Path, target: &Path) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            fs::create_dir(&destination)?;
            copy(&entry.path(), &destination)?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }

    Ok(())
}

fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace('\\', "/").replace(root, "$ROOT"),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(value, root)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(value, root)),
        _ => {}
    }
}

#[test]
fn configuration_projects() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project");

    let mut cases = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;

    cases.sort();

    for case in cases {
        let directory = Directory::new()?;
        copy(&case.join("input"), &directory.0)?;

        let relative = match fs::read_to_string(case.join("path.txt")) {
            Ok(path) => path,
            Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };

        let result = Project::load(&directory.0.join(relative.trim()), Duration::from_secs(2));

        if case.join("error.txt").exists() {
            let expected = fs::read_to_string(case.join("error.txt"))?;
            let error = result.err().expect("invalid fixture must fail").to_string();

            assert!(
                error.contains(expected.trim()),
                "{}: {error}",
                case.display()
            );

            assert!(
                error.contains(&directory.0.to_string_lossy().to_string()),
                "{}: error must name its origin: {error}",
                case.display()
            );
        } else {
            let project = result?;
            let mut output = json!({"configuration": project.configuration, "native": project.native.snapshot()?});

            normalize(
                &mut output,
                &directory.0.to_string_lossy().replace('\\', "/"),
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
