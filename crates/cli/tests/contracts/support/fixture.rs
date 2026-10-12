use std::{
    fs,
    io::{self, Write as _},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Once},
};

use instar_project::Project;
use instar_shared::process;
use serde_json::Value;
use tempfile::TempDir;

static HOST: Once = Once::new();

pub(crate) struct Fixture {
    directory: TempDir,
}

pub(crate) struct Output {
    pub(crate) status: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

impl Fixture {
    pub(crate) fn new(owner: &str, scenario: &str) -> Self {
        let fixture = Self::empty();

        let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("CLI crate has a workspace parent");

        let source = crates.join(owner).join("tests/fixtures").join(scenario);
        assert!(source.is_dir(), "missing fixture: {}", source.display());
        copy(&source, fixture.root(), fixture.root());

        fixture
    }

    pub(crate) fn empty() -> Self {
        Self {
            directory: tempfile::tempdir().expect("create private fixture project"),
        }
    }

    pub(crate) fn root(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn path(&self, name: &str) -> PathBuf {
        let mut path = self.root().to_path_buf();

        for component in Path::new(name).components() {
            match component {
                Component::Normal(component) => path.push(component),
                Component::CurDir => {}
                _ => panic!("fixture paths must remain within their private project"),
            }
        }

        path
    }

    pub(crate) fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);

        fs::create_dir_all(path.parent().expect("fixture file has a parent"))
            .expect("create fixture source directory");

        fs::write(&path, bytes).expect("write fixture source");

        path
    }

    pub(crate) fn read(&self, name: &str) -> String {
        fs::read_to_string(self.path(name)).expect("read fixture source text")
    }

    pub(crate) fn bytes(&self, name: &str) -> Vec<u8> {
        fs::read(self.path(name)).expect("read exact fixture source bytes")
    }

    pub(crate) fn uri(&self, name: &str) -> String {
        let path = self.path(name).to_string_lossy().replace('\\', "/");
        let mut uri = String::from("file:///");

        for byte in path.trim_start_matches('/').bytes() {
            match byte {
                b'A'..=b'Z'
                | b'a'..=b'z'
                | b'0'..=b'9'
                | b'-'
                | b'.'
                | b'_'
                | b'~'
                | b'/'
                | b':' => uri.push(char::from(byte)),

                _ => {
                    use std::fmt::Write as _;
                    write!(uri, "%{byte:02X}").expect("encode fixture URI");
                }
            }
        }

        uri
    }

    pub(crate) fn project(&self) -> Arc<Project> {
        HOST.call_once(|| {
            process::set_host(Path::new(env!("CARGO_BIN_EXE_instar")))
                .expect("select the real native worker executable");
        });

        Arc::new(Project::new(vec![self.root().to_path_buf()]))
    }

    pub(crate) fn run(&self, arguments: &[&str]) -> Output {
        self.execute(arguments, None)
    }

    pub(crate) fn stdin(&self, arguments: &[&str], source: &str) -> Output {
        self.execute(arguments, Some(source))
    }

    pub(crate) fn normalize(&self, value: &Value) -> Value {
        let root = self.root().to_string_lossy();
        let slash = root.replace('\\', "/");
        let uri = self.uri("");

        normalize(value, &root, &slash, &uri)
    }

    fn execute(&self, arguments: &[&str], source: Option<&str>) -> Output {
        let mut command = Command::new(env!("CARGO_BIN_EXE_instar"));
        command.current_dir(self.root());

        if !arguments
            .iter()
            .any(|argument| *argument == "--timeout" || argument.starts_with("--timeout="))
        {
            command.args(["--timeout", "20"]);
        }

        command
            .args(arguments)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        command.stdin(if source.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });

        let mut child = command.spawn().expect("start the real command executable");

        if let Some(source) = source {
            let mut input = child.stdin.take().expect("command stdin was requested");

            match input.write_all(source.as_bytes()) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => {}
                Err(error) => panic!("write command input: {error}"),
            }
        }

        let result = child.wait_with_output().expect("collect command outcome");

        Output {
            status: result.status.code().expect("command has an exit status"),
            stdout: String::from_utf8(result.stdout).expect("command stdout is UTF-8"),
            stderr: String::from_utf8(result.stderr).expect("command stderr is UTF-8"),
        }
    }
}

impl Output {
    pub(crate) fn json(&self) -> Value {
        let text = if self.stdout.trim().is_empty() {
            &self.stderr
        } else {
            &self.stdout
        };

        serde_json::from_str(text).unwrap_or_else(|error| {
            panic!(
                "expected a JSON command report: {error}; exit {}; stdout {:?}; stderr {:?}",
                self.status, self.stdout, self.stderr
            )
        })
    }
}

fn copy(source: &Path, destination: &Path, root: &Path) {
    fs::create_dir_all(destination).expect("create fixture directory");

    let mut entries = fs::read_dir(source)
        .expect("read fixture directory")
        .collect::<io::Result<Vec<_>>>()
        .expect("read fixture entries");

    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let target = destination.join(entry.file_name());
        let kind = entry.file_type().expect("read fixture entry kind");

        assert!(
            !kind.is_symlink(),
            "fixture inputs must be real project files"
        );

        if kind.is_dir() {
            copy(&path, &target, root);
        } else {
            let bytes = fs::read(&path).expect("read fixture file");

            if bytes.windows(5).any(|window| window == b"$ROOT") {
                let text = String::from_utf8(bytes).expect("root template is UTF-8");

                fs::write(
                    target,
                    text.replace("$ROOT", &root.to_string_lossy().replace('\\', "/")),
                )
                .expect("materialize fixture root template");
            } else {
                fs::write(target, bytes).expect("copy exact fixture bytes");
            }
        }
    }
}

fn normalize(value: &Value, root: &str, slash: &str, uri: &str) -> Value {
    match value {
        Value::String(text) => {
            let normalized = text
                .replace(uri, "$ROOT")
                .replace(root, "$ROOT")
                .replace(slash, "$ROOT");

            let normalized = if normalized.starts_with("$ROOT") {
                normalized.replace('\\', "/")
            } else {
                normalized
            };

            Value::String(normalized)
        }

        Value::Array(values) => Value::Array(
            values
                .iter()
                .map(|value| normalize(value, root, slash, uri))
                .collect(),
        ),

        Value::Object(values) => Value::Object(
            values
                .iter()
                .filter(|(key, value)| match key.as_str() {
                    "resultId" => false,
                    "identity" | "context" | "source" | "revision" | "Source" => !value.is_number(),

                    "id" => {
                        !values.contains_key("jsonrpc")
                            && !values
                                .get("method")
                                .is_some_and(|method| method == "workspace/didChangeWatchedFiles")
                    }

                    "data" => value.get("identity").is_none() || value.get("session").is_none(),

                    "arguments" => !value.as_array().is_some_and(|arguments| {
                        arguments.iter().any(|argument| {
                            argument.get("identity").is_some() && argument.get("session").is_some()
                        })
                    }),

                    "uri" => !value
                        .as_str()
                        .is_some_and(|uri| uri.starts_with("instar://definition/")),

                    _ => true,
                })
                .map(|(key, value)| (key.clone(), normalize(value, root, slash, uri)))
                .collect(),
        ),

        _ => value.clone(),
    }
}
