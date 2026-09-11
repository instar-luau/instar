use super::{CONFIG_FILES, ConfigFile, ConfigKind, Project, ProjectError, selection::Selection};
use crate::{configuration::InstarConfig, luau, source::absolute};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub(crate) struct Configuration {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    aliases: BTreeMap<String, String>,
}

#[derive(Clone)]
pub(super) struct Alias {
    pub(super) configuration: PathBuf,
    pub(super) value: String,
}

#[derive(Default)]
pub(crate) struct Discovery {
    configurations: BTreeMap<PathBuf, Vec<Configuration>>,
    aliases: BTreeMap<PathBuf, BTreeMap<String, Alias>>,
    contents: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

impl Discovery {
    fn contents(&mut self, path: &Path) -> io::Result<Option<&[u8]>> {
        if !self.contents.contains_key(path) {
            let exists = match fs::symlink_metadata(path) {
                Ok(_) => true,

                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    false
                }

                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ));
                }
            };

            let contents = if exists {
                Some(fs::read(path).map_err(|error| {
                    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                })?)
            } else {
                None
            };

            self.contents.insert(path.to_owned(), contents);
        }

        Ok(self.contents[path].as_deref())
    }

    pub(crate) fn configurations(&mut self, from: &Path) -> io::Result<&[Configuration]> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        if !self.configurations.contains_key(directory) {
            let mut configurations = Vec::new();

            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                self.project_aliases(ancestor)?;

                let executable = ancestor.join(".config.luau");

                if self.contents(&executable)?.is_some() {
                    return Err(io::Error::other(format!(
                        "{}: executable configuration is not supported by the native integration",
                        executable.display()
                    )));
                }

                let path = ancestor.join(".luaurc");

                let Some(bytes) = self.contents(&path)? else {
                    continue;
                };

                let bytes = bytes.to_vec();

                let aliases = luau::aliases(&bytes)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

                configurations.push(Configuration {
                    path,
                    bytes,
                    aliases,
                });
            }

            self.configurations
                .insert(directory.to_owned(), configurations);
        }

        Ok(&self.configurations[directory])
    }

    pub(crate) fn definitions(&mut self, from: &Path) -> io::Result<Vec<PathBuf>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent"))?;

        let mut definitions = Vec::new();

        for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
            let path = ancestor.join("instar.toml");

            let Some(contents) = self.contents(&path)? else {
                continue;
            };

            let contents = std::str::from_utf8(contents).map_err(io::Error::other)?;

            let configuration = InstarConfig::parse(contents)
                .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

            if let Some(configured) = configuration.definitions {
                definitions = configured
                    .into_iter()
                    .map(|path| absolute(&ancestor.join(path)).map_err(io::Error::other))
                    .collect::<io::Result<Vec<_>>>()?;
            }
        }

        let mut seen = std::collections::BTreeSet::new();
        definitions.retain(|path| seen.insert(path.clone()));

        Ok(definitions)
    }

    fn project_aliases(&mut self, directory: &Path) -> io::Result<&BTreeMap<String, Alias>> {
        if !self.aliases.contains_key(directory) {
            let mut aliases = BTreeMap::new();

            let path = directory.join("instar.toml");

            if let Some(contents) = self.contents(&path)? {
                let contents = std::str::from_utf8(contents)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

                let configuration = InstarConfig::parse(contents)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

                let configured = configuration.aliases.unwrap_or_default();
                let validation = serde_json::to_vec(&serde_json::json!({"aliases": &configured}))?;

                luau::aliases(&validation)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;

                for (alias, target) in configured {
                    let key = alias.to_ascii_lowercase();

                    let value = target
                        .to_str()
                        .ok_or_else(|| io::Error::other("alias path requires UTF-8"))?
                        .to_owned();

                    if aliases
                        .insert(
                            key,
                            Alias {
                                configuration: path.clone(),
                                value,
                            },
                        )
                        .is_some()
                    {
                        return Err(io::Error::other(format!(
                            "{}: duplicate case-insensitive alias {alias}",
                            path.display()
                        )));
                    }
                }
            }

            self.aliases.insert(directory.to_owned(), aliases);
        }

        Ok(&self.aliases[directory])
    }

    pub(super) fn alias(&mut self, from: &Path, name: &str) -> io::Result<Option<Alias>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        self.configurations(from)?;

        for ancestor in directory.ancestors() {
            if let Some(alias) = self.project_aliases(ancestor)?.get(name) {
                return Ok(Some(alias.clone()));
            }

            if let Some(alias) = self.configurations[directory]
                .iter()
                .find_map(|configuration| {
                    if configuration.path.parent() != Some(ancestor) {
                        return None;
                    }

                    Some(Alias {
                        configuration: configuration.path.clone(),
                        value: configuration.aliases.get(name)?.clone(),
                    })
                })
            {
                return Ok(Some(alias));
            }
        }

        Ok(None)
    }
}

impl Project {
    /// # Errors
    /// Returns root, filesystem, encoding or Instar configuration failures.
    pub fn load(root: &Path) -> Result<Self, ProjectError> {
        let root = std::path::absolute(root).map_err(|source| ProjectError::Io {
            path: root.to_owned(),
            source,
        })?;

        let metadata = fs::metadata(&root).map_err(|source| ProjectError::Io {
            path: root.clone(),
            source,
        })?;

        if !metadata.is_dir() {
            return Err(ProjectError::NotDirectory(root));
        }

        let mut files = Vec::new();
        let mut instar = None;

        for (name, kind) in CONFIG_FILES {
            let path = root.join(name);

            match fs::symlink_metadata(&path) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => return Err(ProjectError::Io { path, source }),
            }

            let metadata = fs::metadata(&path).map_err(|source| ProjectError::Io {
                path: path.clone(),
                source,
            })?;

            if !metadata.is_file() {
                return Err(ProjectError::NotFile(path));
            }

            let bytes = fs::read(&path).map_err(|source| ProjectError::Io {
                path: path.clone(),
                source,
            })?;

            if kind == ConfigKind::Instar {
                let text =
                    std::str::from_utf8(&bytes).map_err(|source| ProjectError::Encoding {
                        path: path.clone(),
                        source,
                    })?;

                instar = Some(
                    InstarConfig::parse(text).map_err(|source| ProjectError::Toml {
                        path: path.clone(),
                        source,
                    })?,
                );
            }

            files.push(ConfigFile { path, kind, bytes });
        }

        Ok(Self {
            root,
            files,
            instar,
        })
    }
}

impl super::Configuration {
    /// # Errors
    /// Returns filesystem, encoding, configuration, or glob failures.
    pub fn discover(path: &Path, explicit: Option<&Path>) -> io::Result<Self> {
        let path = crate::source::absolute(path).map_err(io::Error::other)?;
        let mut merged = serde_json::Map::new();
        let mut selection = Selection::default();
        let mut grafts = BTreeMap::new();

        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("source has no parent"))?;

        if explicit.is_none() {
            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                let configuration = ancestor.join("instar.toml");

                match fs::read_to_string(&configuration) {
                    Ok(text) => {
                        merge(
                            &mut merged,
                            &text,
                            &mut selection,
                            &mut grafts,
                            &configuration,
                        )
                        .map_err(|error| {
                            io::Error::other(format!("{}: {error}", configuration.display()))
                        })?;
                    }

                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                }
            }
        }

        if let Some(explicit) = explicit {
            let explicit = crate::source::absolute(explicit).map_err(io::Error::other)?;

            merge(
                &mut merged,
                &fs::read_to_string(&explicit)?,
                &mut selection,
                &mut grafts,
                &explicit,
            )?;
        }

        Ok(Self {
            options: serde_json::from_value(merged.into()).map_err(io::Error::other)?,
            selection,
            grafts: grafts
                .into_iter()
                .map(|(name, path)| crate::graft::Graft::load(&path, &name))
                .collect::<io::Result<Vec<_>>>()?,
        })
    }
}

fn overlay(
    under: &mut serde_json::Map<String, serde_json::Value>,
    over: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in over {
        match (under.get_mut(&key), value) {
            (Some(serde_json::Value::Object(under)), serde_json::Value::Object(over)) => {
                overlay(under, over);
            }

            (_, value) => {
                under.insert(key, value);
            }
        }
    }
}

fn merge(
    merged: &mut serde_json::Map<String, serde_json::Value>,
    text: &str,
    selection: &mut Selection,
    grafts: &mut BTreeMap<String, PathBuf>,
    configuration: &Path,
) -> io::Result<()> {
    crate::configuration::InstarConfig::parse(text).map_err(io::Error::other)?;
    let mut value: serde_json::Value = toml_edit::de::from_str(text).map_err(io::Error::other)?;
    selection.merge(&value, configuration)?;

    if let Some(entries) = value.get("grafts").and_then(serde_json::Value::as_object) {
        for (name, path) in entries {
            let path = path
                .as_str()
                .ok_or_else(|| io::Error::other("graft manifest path must be a string"))?;

            grafts.insert(
                name.clone(),
                configuration
                    .parent()
                    .ok_or_else(|| io::Error::other("configuration has no parent"))?
                    .join(path),
            );
        }
    }

    value = value
        .get_mut("format")
        .map_or(serde_json::Value::Null, serde_json::Value::take);

    if let serde_json::Value::Object(fields) = value {
        overlay(merged, fields);
    }

    Ok(())
}
