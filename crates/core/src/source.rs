//! Cached filesystem and editor source snapshots.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use crate::project::Change;

/// One immutable source revision.
#[derive(Clone, Debug)]
pub struct Document {
    /// Monotonic revision in this view.
    pub revision: u64,

    /// Source bytes decoded as UTF-8.
    pub text: Rc<str>,
}

/// A retained filesystem or configuration failure.
#[derive(Clone, Debug, PartialEq, Eq)]
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

impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Failure {}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Directory,
    Other,
}

#[derive(Default)]
pub(crate) struct View {
    overlays: BTreeMap<PathBuf, Option<Rc<str>>>,
    documents: BTreeMap<PathBuf, Result<Option<Document>, Failure>>,
    kinds: BTreeMap<PathBuf, Result<Option<Kind>, Failure>>,
    revision: u64,
    pub(crate) consulted: BTreeSet<PathBuf>,
}

impl View {
    pub(crate) fn read(&mut self, path: &Path) -> io::Result<Option<Document>> {
        self.consulted.insert(path.to_path_buf());

        if let Some(document) = self.documents.get(path) {
            return document.clone().map_err(|error| error.error());
        }

        let result = if let Some(overlay) = self.overlays.get(path) {
            Ok(overlay.clone())
        } else {
            match fs::read_to_string(path) {
                Ok(text) => Ok(Some(Rc::from(text))),

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

        if let Some(overlay) = self.overlays.get(path) {
            return Ok(overlay.as_ref().map(|_| Kind::File));
        }

        if self
            .overlays
            .iter()
            .any(|(file, text)| text.is_some() && file != path && file.starts_with(path))
        {
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

    pub(crate) fn change(&mut self, path: &Path, change: Change) {
        match change {
            Change::Overlay { text, .. } => {
                self.overlays.insert(path.to_path_buf(), text.map(Rc::from));
            }

            Change::Close(_) => {
                self.overlays.remove(path);
            }

            Change::Disk(_) => {}
        }

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
