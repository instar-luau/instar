use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::Deserialize;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstarConfig {
    /// Formatter settings. Ancestor settings merge field by field; absent settings use their documented defaults.
    #[schemars(default)]
    pub format: Option<crate::format::Options>,

    /// Graft names mapped to manifest paths relative to this configuration. Entries inherit by name and run in name order.
    #[schemars(default)]
    pub grafts: Option<BTreeMap<String, PathBuf>>,

    /// Source selection patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub include: Option<Vec<String>>,

    /// Source exclusion patterns relative to this configuration. Explicit file inputs bypass selection; an empty list clears inherited patterns.
    #[schemars(default)]
    pub exclude: Option<Vec<String>>,

    /// External Luau definition files, resolved relative to this configuration.
    #[schemars(default)]
    pub definitions: Option<Vec<PathBuf>>,

    /// Require aliases mapped to paths relative to this configuration. Entries inherit by name.
    #[schemars(default)]
    pub aliases: Option<BTreeMap<String, PathBuf>>,

    /// Roblox project inputs. Project mapping integration is not yet implemented.
    #[schemars(default)]
    pub roblox: Option<RobloxConfig>,
}

#[derive(Debug, Deserialize, JsonSchema, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct RobloxConfig {
    /// Rojo project path relative to this configuration. Project mapping integration is not yet implemented.
    #[schemars(default)]
    pub project: Option<PathBuf>,

    /// Existing instance-to-source mapping path relative to this configuration. Sourcemap integration is not yet implemented; Instar does not generate this file.
    #[schemars(default)]
    pub sourcemap: Option<PathBuf>,
}

impl InstarConfig {
    /// # Errors
    /// Returns invalid TOML, unknown fields or field-type errors with TOML spans.
    pub fn parse(text: &str) -> Result<Self, toml_edit::de::Error> {
        toml_edit::de::from_str(text)
    }

    #[must_use]
    pub fn schema() -> schemars::Schema {
        let mut schema = schemars::schema_for!(Self);

        if let Some(object) = schema.as_object_mut() {
            for value in object.values_mut() {
                describe(value);
            }
        }

        schema
    }
}

fn describe(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            if let Some(default) = object.get("default") {
                let default = if default.is_null() {
                    "unset".to_owned()
                } else {
                    default.to_string()
                };

                if let Some(serde_json::Value::String(description)) = object.get_mut("description")
                {
                    description.push_str("\n\nDefault: `");
                    description.push_str(&default);
                    description.push_str("`.");
                }
            }

            for value in object.values_mut() {
                describe(value);
            }
        }

        serde_json::Value::Array(values) => {
            for value in values {
                describe(value);
            }
        }

        _ => {}
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigKind {
    Instar,
    Luaurc,
    Luau,
}

const CONFIG_FILES: [(&str, ConfigKind); 4] = [
    (".luaurc", ConfigKind::Luaurc),
    (".config.luau", ConfigKind::Luau),
    ("config.luau", ConfigKind::Luau),
    ("instar.toml", ConfigKind::Instar),
];

impl ConfigKind {
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?;

        CONFIG_FILES
            .iter()
            .find_map(|&(name, kind)| (filename == name).then_some(kind))
    }
}

#[derive(Debug)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub kind: ConfigKind,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    files: Vec<ConfigFile>,
    instar: Option<InstarConfig>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("{path}: {source}")]
    Encoding {
        path: PathBuf,
        #[source]
        source: std::str::Utf8Error,
    },

    #[error("{path}: {source}")]
    Toml {
        path: PathBuf,
        #[source]
        source: toml_edit::de::Error,
    },

    #[error("project root is not a directory: {0}")]
    NotDirectory(PathBuf),

    #[error("configuration is not a regular file: {0}")]
    NotFile(PathBuf),
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

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn files(&self) -> &[ConfigFile] {
        &self.files
    }

    #[must_use]
    pub const fn instar(&self) -> Option<&InstarConfig> {
        self.instar.as_ref()
    }
}
