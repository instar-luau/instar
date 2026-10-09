use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
};

use instar_analysis::{Location, Options, error::invalid};

use crate::{
    project::Project,
    resolve::{Identity, Module},
    source::Document,
};

/// An explicit analysis entry with an optional exact sourcemap placement.
#[derive(Clone, Debug)]
pub struct Entry {
    /// Absolute backing source path.
    pub source: PathBuf,

    /// Explicit identity when the source has multiple mapped placements.
    pub context: Option<Identity>,
}

impl Entry {
    /// Creates a source entry without choosing a mapped placement.
    #[must_use]
    pub fn new(source: PathBuf) -> Self {
        Self {
            source,
            context: None,
        }
    }
}

/// The exact source owner of an analysis location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A module in the Rust-owned graph, retaining its contextual identity.
    Module(Module),

    /// An environment declaration file.
    Definition(PathBuf),
}

impl Origin {
    /// Returns the actual backing source path.
    #[must_use]
    pub fn source(&self) -> &Path {
        match self {
            Self::Module(module) => &module.source,
            Self::Definition(path) => path,
        }
    }
}

impl Project {
    /// Expands source paths into every mapped analysis context.
    ///
    /// # Errors
    /// Returns source, configuration, resolution, or interruption failures.
    pub fn entries(&mut self, paths: &[PathBuf], options: &Options) -> io::Result<Vec<Entry>> {
        self.during(
            |project| {
                let mut entries = Vec::new();

                for path in paths {
                    project.interrupted()?;

                    entries.extend(project.contexts(path)?.into_iter().map(|module| Entry {
                        source: module.source,
                        context: Some(module.identity),
                    }));
                }

                Ok(entries)
            },
            options,
        )
    }
}

pub(crate) fn locate(
    location: &Location<String>,
    sources: &BTreeMap<String, (Origin, Document)>,
) -> io::Result<Location<Origin>> {
    let (origin, document) = sources
        .get(&location.module)
        .ok_or_else(|| invalid("native location has an unknown host identity"))?;

    if document.revision != location.revision
        || location.range[0] > location.range[1]
        || location.range[1] > document.text.len()
        || !document.text.is_char_boundary(location.range[0])
        || !document.text.is_char_boundary(location.range[1])
    {
        return Err(invalid(
            "native location is not anchored to its host revision",
        ));
    }

    Ok(Location {
        module: origin.clone(),
        revision: location.revision,
        range: location.range,
    })
}
