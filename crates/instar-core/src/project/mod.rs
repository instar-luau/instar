mod discovery;
pub mod resolution;
pub mod selection;

use crate::configuration::{InstarConfig, format::Options};
use selection::Selection;
use std::{
    io,
    path::{Path, PathBuf},
};

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

pub struct Configuration {
    pub options: Options,
    pub selection: Selection,
    pub grafts: Vec<crate::graft::Graft>,
}

impl Configuration {
    /// # Errors
    /// Returns native formatting failures, graft traps, or invalid graft output.
    pub fn format(&self, source: &[u8]) -> io::Result<Vec<u8>> {
        let mut output = crate::format::format(source, &self.options)?;

        if self.options.enabled {
            for graft in &self.grafts {
                output = graft.format(&output, &self.options)?;
            }
        }

        Ok(output)
    }
}
