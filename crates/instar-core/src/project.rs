//! Project configuration at an explicitly selected root.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::Deserialize;

/// Instar project inputs. Paths are relative to the containing configuration file.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstarConfig {
    /// Glob patterns selecting entry files for commands.
    pub include: Option<Vec<String>>,
    /// Glob patterns excluding entry files, not required dependencies.
    pub exclude: Option<Vec<String>>,
    /// External Luau type-definition files.
    pub definitions: Option<Vec<PathBuf>>,
    /// Logical require aliases mapped to paths relative to this configuration.
    pub aliases: Option<BTreeMap<String, PathBuf>>,
    /// Optional Roblox project inputs.
    pub roblox: Option<RobloxConfig>,
}

/// Existing Roblox project inputs; loading configuration does not regenerate them.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RobloxConfig {
    /// Rojo project file.
    pub project: Option<PathBuf>,
    /// Existing instance-to-source mapping.
    pub sourcemap: Option<PathBuf>,
}

impl InstarConfig {
    /// Decode the supported fields without assigning defaults to omitted values.
    ///
    /// # Errors
    /// Returns invalid TOML, unknown fields or field-type errors with TOML spans.
    pub fn parse(text: &str) -> Result<Self, toml_edit::de::Error> {
        toml_edit::de::from_str(text)
    }

    /// Generate the editor schema from the same types used to decode configuration.
    #[must_use]
    pub fn schema() -> schemars::Schema {
        schemars::schema_for!(Self)
    }
}

/// Recognized project configuration formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigKind {
    /// Instar TOML configuration.
    Instar,
    /// Upstream JSON configuration.
    Luaurc,
    /// Luau configuration source retained without evaluation.
    Luau,
}

const CONFIG_FILES: [(&str, ConfigKind); 4] = [
    (".luaurc", ConfigKind::Luaurc),
    (".config.luau", ConfigKind::Luau),
    ("config.luau", ConfigKind::Luau),
    ("instar.toml", ConfigKind::Instar),
];

impl ConfigKind {
    /// Recognize an exact configuration filename.
    #[must_use]
    pub fn from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?;
        CONFIG_FILES
            .iter()
            .find_map(|&(name, kind)| (filename == name).then_some(kind))
    }
}

/// Original configuration bytes and their owning file.
#[derive(Debug)]
pub struct ConfigFile {
    pub path: PathBuf,
    pub kind: ConfigKind,
    pub bytes: Vec<u8>,
}

/// Independently retained configurations at one selected project root.
#[derive(Debug)]
pub struct Project {
    root: PathBuf,
    files: Vec<ConfigFile>,
    instar: Option<InstarConfig>,
}

/// Configuration acquisition and decoding failures retain their file owner.
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
    /// Load recognized files directly under a selected root, retaining coexistence.
    /// No ancestor selection, cross-file merging or Luau evaluation is performed.
    ///
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
            // Inspect presence separately so a dangling link is an error, not absence.
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

    /// Base directory for relative configuration paths.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every recognized configuration, without assigning cross-format precedence.
    #[must_use]
    pub fn files(&self) -> &[ConfigFile] {
        &self.files
    }

    /// Parsed Instar settings, absent when no Instar configuration exists.
    #[must_use]
    pub const fn instar(&self) -> Option<&InstarConfig> {
        self.instar.as_ref()
    }
}
