//! Cached filesystem and editor source snapshots.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use instar_analysis::Options;

use crate::{project::Change, snapshot::Snapshot};

/// One immutable source revision.
#[derive(Clone, Debug)]
pub struct Document {
    /// Monotonic revision in this view.
    pub revision: u64,

    /// Source bytes decoded as UTF-8.
    pub text: Arc<str>,
}

/// A retained filesystem or configuration failure.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    /// Original error category.
    pub kind: io::ErrorKind,

    /// Source-qualified explanation.
    pub message: String,
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl From<Failure> for io::Error {
    fn from(error: Failure) -> Self {
        Self::new(error.kind, error.message)
    }
}

impl Failure {
    pub(crate) fn error(&self) -> io::Error {
        io::Error::new(self.kind, self.message.clone())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Directory,
    Other,
}

#[derive(Default)]
pub(crate) struct View {
    pub(crate) snapshot: Snapshot,
    documents: BTreeMap<PathBuf, Result<Option<Document>, Failure>>,
    kinds: BTreeMap<PathBuf, Result<Option<Kind>, Failure>>,
    revision: u64,
    pub(crate) consulted: BTreeSet<PathBuf>,
}

impl View {
    pub(crate) fn consult<'paths>(&mut self, paths: impl IntoIterator<Item = &'paths PathBuf>) {
        for path in paths {
            if !self.consulted.contains(path) {
                self.consulted.insert(path.clone());
            }
        }
    }

    pub(crate) fn frozen(&self, options: &Options, started: Instant) -> io::Result<Snapshot> {
        let mut snapshot = self.snapshot.clone();

        for (path, source) in &self.documents {
            options.check(started)?;

            if let Ok(source) = source {
                snapshot.change(Change::Overlay {
                    path: path.clone(),
                    text: source.as_ref().map(|source| source.text.to_string()),
                })?;
            }
        }

        Ok(snapshot)
    }

    pub(crate) fn read(&mut self, path: &Path) -> io::Result<Option<Document>> {
        self.consulted.insert(path.to_path_buf());

        if let Some(document) = self.documents.get(path) {
            return document.clone().map_err(|error| error.error());
        }

        let result = if let Some(overlay) = self.snapshot.overlay(path) {
            Ok(overlay.clone())
        } else {
            match fs::read_to_string(path) {
                Ok(text) => Ok(Some(Arc::from(text))),

                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    Ok(None)
                }

                Err(error) => Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", path.display()),
                )),
            }
        }
        .map(|text| {
            text.map(|text| {
                self.revision += 1;

                Document {
                    revision: self.revision,
                    text,
                }
            })
        })
        .map_err(Failure::from);

        self.documents.insert(path.to_path_buf(), result.clone());

        result.map_err(|error| error.error())
    }

    pub(crate) fn kind(&mut self, path: &Path) -> io::Result<Option<Kind>> {
        self.consulted.insert(path.to_path_buf());

        if let Some(overlay) = self.snapshot.overlay(path) {
            return Ok(overlay.as_ref().map(|_| Kind::File));
        }

        if self.snapshot.directory(path) {
            return Ok(Some(Kind::Directory));
        }

        if let Some(kind) = self.kinds.get(path) {
            return kind.clone().map_err(|error| error.error());
        }

        let result = match fs::metadata(path) {
            Ok(metadata) => Ok(Some(if metadata.is_file() {
                Kind::File
            } else if metadata.is_dir() {
                Kind::Directory
            } else {
                Kind::Other
            })),

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                Ok(None)
            }

            Err(error) => Err(Failure::from(io::Error::new(
                error.kind(),
                format!("{}: {error}", path.display()),
            ))),
        };

        self.kinds.insert(path.to_path_buf(), result.clone());

        result.map_err(|error| error.error())
    }

    pub(crate) fn changes(&self, path: &Path) -> BTreeSet<PathBuf> {
        let mut changed = BTreeSet::from([path.to_path_buf()]);

        changed.extend(
            self.kinds
                .iter()
                .filter(|(candidate, kind)| path.starts_with(candidate) && matches!(kind, Ok(None)))
                .map(|(candidate, _)| candidate.clone()),
        );

        changed.extend(
            self.documents
                .iter()
                .filter(|(candidate, document)| {
                    path.starts_with(candidate) && matches!(document, Ok(None))
                })
                .map(|(candidate, _)| candidate.clone()),
        );

        changed
    }

    pub(crate) fn invalidate(&mut self, path: &Path) {
        self.documents
            .retain(|candidate, _| !related(candidate, path) && !related(path, candidate));

        self.kinds
            .retain(|candidate, _| !related(candidate, path) && !related(path, candidate));
    }
}

pub(crate) fn related(candidate: &Path, changed: &Path) -> bool {
    candidate == changed || candidate.starts_with(changed)
}

pub(crate) fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            component => normalized.push(component.as_os_str()),
        }
    }

    normalized
}
