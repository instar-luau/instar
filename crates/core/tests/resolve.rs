//! Synthetic require resolution fixtures with independent expected outcomes.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use instar_core::resolve::resolve;
use serde::Deserialize;
use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "instar-require-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));

        fs::create_dir(&path)?;

        Ok(Self(path))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("remove synthetic require fixture");
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    source: String,
    request: String,
}

fn copy(source: &Path, target: &Path, root: &str) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            fs::create_dir(&destination)?;
            copy(&entry.path(), &destination, root)?;
        } else {
            fs::write(
                destination,
                fs::read_to_string(entry.path())?.replace("$ROOT", root),
            )?;
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
fn require_fixtures() -> Result<(), Box<dyn std::error::Error>> {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/resolve");

    let mut cases = fs::read_dir(fixtures)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;

    cases.sort();

    for case in cases {
        let directory = Directory::new()?;
        let root = directory.0.to_string_lossy().replace('\\', "/");
        copy(&case.join("input"), &directory.0, &root)?;

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
            let source = directory.0.join(&request.source);
            let argument = request.request.replace("$ROOT", &root);

            let mut output = match resolve(&source, &argument, Duration::from_secs(2)) {
                Ok(module) => json!({"module": {"path": module.path, "source": module.source}}),
                Err(error) => json!({"error": error.to_string()}),
            };

            normalize(&mut output, &root);

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
    let error = resolve(Path::new("caller.luau"), "./target", Duration::from_secs(2))
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
            r#"{"aliases":{"SeLf":"./target"}}"#,
            "alias @self is reserved",
        ),
        (
            ".config.luau",
            "return {luau = {aliases = {SELF = './target'}}}",
            "alias @self is reserved",
        ),
        (
            ".luaurc",
            r#"{"aliases":{"@named":"./target"}}"#,
            "Invalid alias @named",
        ),
        (
            ".config.luau",
            "return {luau = {aliases = {['@named'] = './target'}}}",
            "Invalid alias @named",
        ),
    ] {
        let mut native = instar_bridge::Configuration::new()?;

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
