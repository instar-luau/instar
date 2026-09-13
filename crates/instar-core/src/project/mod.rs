mod discovery;

/// Require path resolution using source and configuration snapshots.
pub mod resolution;

/// Inherited source inclusion and exclusion patterns.
pub mod selection;

use crate::configuration::format::Options;
use selection::Selection;
use std::{io, path::Path};

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
