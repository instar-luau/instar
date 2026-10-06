use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use instar_analysis::Options;
use instar_core::project::Project;
use serde_json::Value;

static NEXT: AtomicUsize = AtomicUsize::new(0);

pub(crate) struct Directory {
    pub(crate) path: PathBuf,
}

impl Directory {
    pub(crate) fn new(configuration: Option<&str>) -> io::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "instar-project-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));

        fs::create_dir(&path)?;
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
    use sha2::{Digest, Sha256};
    let directory = instar_core::roblox::cache_directory()?;

    let mut file = |name: &str, text: &str| {
        project
            .change(instar_core::project::Change::Overlay {
                path: directory.join(name),
                text: Some(text.to_owned()),
            })
            .map(|_| ())
    };

    let declarations = "declare extern type Instance with\n    read Name: string\nend\ndeclare extern type DataModel extends Instance with end\ndeclare extern type Part extends Instance with end\ndeclare Instance: {new: (name: string) -> Instance}\ndeclare game: DataModel\ndeclare script: Instance";
    let plugin = format!("{declarations}\ndeclare plugin: {{Name: string}}");
    let mut profiles = serde_json::Map::new();

    for (name, source) in [
        ("none", declarations),
        ("local", declarations),
        ("plugin", plugin.as_str()),
        ("roblox", plugin.as_str()),
    ] {
        file(&format!("{name}.d.luau"), source)?;

        profiles.insert(
            name.to_owned(),
            Value::String(format!("{:x}", Sha256::digest(source.as_bytes()))),
        );
    }

    file("enumerations.d.luau", "")?;
    file("documentation.json", "{}")?;

    file(
        "metadata.json",
        &serde_json::json!({
            "revision": "0123456789012345678901234567890123456789",
            "services": ["DataModel"],
            "creatable_instances": ["Part"],
            "profiles": profiles,
            "properties": {"none": [], "local": [], "plugin": [], "roblox": []},
            "enumerations": format!("{:x}", Sha256::digest(b"")),
            "documentation": format!("{:x}", Sha256::digest(b"{}")),
        })
        .to_string(),
    )?;

    Ok(())
}
