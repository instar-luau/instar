use std::{
    collections::BTreeMap,
    fs,
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
    thread,
    time::{Duration, Instant},
};

use instar_project::{
    ChangeBatch, Project, ProjectChange, SourceChange, SourceOrigin, documentation_cache_directory,
    roblox_cache_directory,
};

use instar_shared::Options;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

pub(crate) struct Directory {
    directory: TempDir,
}

impl Directory {
    pub(crate) fn new(configuration: Option<&str>) -> io::Result<Self> {
        let directory = Self {
            directory: tempfile::tempdir()?,
        };

        if let Some(configuration) = configuration {
            directory.file(".luaurc", configuration)?;
        }

        Ok(directory)
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn file(&self, name: &str, text: &str) -> io::Result<PathBuf> {
        let path = self.path().join(name);
        fs::create_dir_all(path.parent().expect("fixture parent"))?;
        fs::write(&path, text)?;

        Ok(path)
    }
}

pub(crate) fn project() -> Project {
    let project = Project::new(Vec::new());
    assets(&project).expect("Roblox cache fixture");

    project
}

pub(crate) fn options() -> Options {
    Options::new(Some(Instant::now() + Duration::from_secs(5)))
}

pub(crate) fn copy(source: &Path, target: &Path, root: Option<&str>) -> io::Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let destination = target.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            fs::create_dir(&destination)?;
            copy(&entry.path(), &destination, root)?;
        } else if let Some(root) = root {
            fs::write(
                destination,
                fs::read_to_string(entry.path())?.replace("$ROOT", root),
            )?;
        } else {
            fs::copy(entry.path(), destination)?;
        }
    }

    Ok(())
}

pub(crate) fn normalize(root: &str, value: &mut Value) {
    match value {
        Value::String(text) => *text = text.replace('\\', "/").replace(root, "$ROOT"),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(root, value)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(root, value)),
        _ => {}
    }
}

pub(crate) fn complete<Output>(future: impl Future<Output = Output>) -> Output {
    struct Notification(thread::Thread);

    impl Wake for Notification {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(Notification(thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);

    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => return output,
            Poll::Pending => thread::park(),
        }
    }
}

pub(crate) fn replace(project: &Project, path: &Path, text: &str) -> io::Result<()> {
    let snapshot = project.snapshot();

    let origin = SourceOrigin::Document {
        address: path.to_string_lossy().into_owned(),
        context: Some(path.to_path_buf()),
    };

    let mut changes = Vec::new();

    if let Some(source) = snapshot.source_at(path) {
        let mut expected = source.version;

        if !matches!(
            snapshot.origin(expected.identity),
            Some(SourceOrigin::Document { .. })
        ) {
            changes.push(ProjectChange::Source(SourceChange::Move {
                expected,
                origin,
            }));

            expected.revision = expected
                .revision
                .checked_add(1)
                .ok_or_else(|| io::Error::other("fixture revision overflow"))?;
        }

        changes.push(ProjectChange::Source(SourceChange::Replace {
            expected,
            text: Arc::from(text),
        }));
    } else {
        changes.push(ProjectChange::Source(SourceChange::Open {
            identity: project.source_identity(),
            origin,
            text: Arc::from(text),
        }));
    }

    project.change(ChangeBatch { changes })?;

    Ok(())
}

fn assets(project: &Project) -> io::Result<()> {
    for (directory, fixture) in [
        (roblox_cache_directory()?, "project"),
        (documentation_cache_directory()?, "documentation"),
    ] {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/assets")
            .join(fixture);

        let files = fs::read_dir(source)?
            .map(|entry| {
                let entry = entry?;

                let name = entry
                    .file_name()
                    .into_string()
                    .map_err(|_| io::Error::other("fixture filename must be UTF-8"))?;

                Ok((name, fs::read_to_string(entry.path())?))
            })
            .collect::<io::Result<BTreeMap<_, _>>>()?;

        if fixture == "project" {
            let metadata: Value = serde_json::from_str(
                files
                    .get("metadata.json")
                    .ok_or_else(|| io::Error::other("fixture metadata is missing"))?,
            )
            .map_err(io::Error::other)?;

            let profiles = metadata
                .get("profiles")
                .and_then(Value::as_object)
                .ok_or_else(|| io::Error::other("fixture metadata profiles are missing"))?;

            for profile in ["none", "local", "plugin", "roblox"] {
                let declarations = files
                    .get(&format!("{profile}.d.luau"))
                    .ok_or_else(|| io::Error::other("fixture declarations are missing"))?;

                let digest = profiles
                    .get(profile)
                    .ok_or_else(|| io::Error::other("fixture profile digest is missing"))?;

                if digest.as_str()
                    != Some(format!("{:x}", Sha256::digest(declarations.as_bytes())).as_str())
                {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "fixture declarations do not match their metadata",
                    ));
                }
            }
        }

        for (name, text) in files {
            replace(project, &directory.join(name), &text)?;
        }
    }

    Ok(())
}
