mod discovery;

/// Require path resolution using source and configuration snapshots.
pub mod resolution;

/// Inherited source inclusion and exclusion patterns.
pub mod selection;

use crate::configuration::format::Options;
use selection::Selection;

use std::{
    io,
    path::{Path, PathBuf},
};

#[must_use]
/// Nearest Instar configuration affecting the path.
pub fn nearest_configuration(path: &Path) -> Option<PathBuf> {
    path.parent()?
        .ancestors()
        .map(|directory| directory.join("instar.toml"))
        .find(|configuration| configuration.is_file())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Supported project configuration formats.
pub enum ConfigKind {
    /// An `instar.toml` configuration.
    Instar,

    /// A declarative `.luaurc` configuration.
    Luaurc,

    /// An executable Luau configuration.
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
    /// Identify a supported configuration by its file name.
    pub fn from_path(path: &Path) -> Option<Self> {
        let filename = path.file_name()?;

        CONFIG_FILES
            .iter()
            .find_map(|&(name, kind)| (filename == name).then_some(kind))
    }
}

/// Resolved formatting settings, source selection, and loaded grafts.
pub struct Configuration {
    /// Effective formatter options after inheritance.
    pub options: Options,

    /// Effective source inclusion and exclusion rules.
    pub selection: Selection,

    /// Formatting grafts in execution order.
    pub grafts: Vec<crate::graft::Graft>,
}

impl Configuration {
    #[must_use]
    /// Whether the path is ordinary Luau or owned by a configured frontend.
    pub fn language(&self, path: &Path) -> bool {
        path.extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|extension| matches!(extension, "lua" | "luau"))
            || self.frontend(path).is_some()
    }

    pub(crate) fn frontend(&self, path: &Path) -> Option<&crate::graft::Graft> {
        self.grafts.iter().find(|graft| graft.owns(path))
    }

    pub(crate) fn frontend_extensions(&self) -> impl Iterator<Item = &str> {
        self.grafts.iter().flat_map(crate::graft::Graft::extensions)
    }

    /// # Errors
    /// Returns native formatting failures, graft traps, or invalid graft output.
    pub fn format(&self, path: &Path, source: &[u8]) -> io::Result<Vec<u8>> {
        if let Some(graft) = self.frontend(path) {
            if !self.options.enabled {
                return Ok(source.to_vec());
            }

            return graft.format(source, &self.options);
        }

        let mut output = crate::format::format(source, &self.options)?;

        if self.options.enabled {
            for graft in self.grafts.iter().filter(|graft| !graft.is_frontend()) {
                output = graft.format(&output, &self.options)?;
            }
        }

        Ok(output)
    }
}
