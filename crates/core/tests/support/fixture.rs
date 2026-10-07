use std::{
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use instar_analysis::Options;
use instar_core::project::Project;
use serde_json::Value;

pub(crate) struct Directory {
    pub(crate) path: PathBuf,
}

impl Directory {
    pub(crate) fn new(configuration: Option<&str>) -> io::Result<Self> {
        let path = tempfile::tempdir()?.keep();
        let directory = Self { path };

        if let Some(configuration) = configuration {
            directory.file(".luaurc", configuration)?;
        }

        Ok(directory)
    }

    pub(crate) fn file(&self, name: &str, text: &str) -> io::Result<PathBuf> {
        let path = self.path.join(name);
        fs::create_dir_all(path.parent().expect("fixture parent"))?;
        fs::write(&path, text)?;

        Ok(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).expect("remove project fixture");
    }
}

pub(crate) fn project() -> Project {
    let mut project = Project::new(Duration::from_secs(2));
    assets(&mut project).expect("Roblox cache fixture");

    project
}

pub(crate) fn options() -> Options {
    Options::new(Duration::from_secs(5))
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

pub(crate) fn normalize(value: &mut Value, root: &str) {
    match value {
        Value::String(text) => *text = text.replace('\\', "/").replace(root, "$ROOT"),
        Value::Array(values) => values.iter_mut().for_each(|value| normalize(value, root)),
        Value::Object(values) => values.values_mut().for_each(|value| normalize(value, root)),
        _ => {}
    }
}

fn assets(project: &mut Project) -> io::Result<()> {
    for (directory, fixture) in [
        (instar_core::roblox::cache_directory()?, "project"),
        (
            instar_core::documentation::cache_directory()?,
            "documentation",
        ),
    ] {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/assets")
            .join(fixture);

        for entry in fs::read_dir(fixture)? {
            let entry = entry?;

            project.change(instar_core::project::Change::Overlay {
                path: directory.join(entry.file_name()),
                text: Some(fs::read_to_string(entry.path())?),
            })?;
        }
    }

    Ok(())
}
