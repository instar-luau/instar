//! Shareable editor inputs and filesystem invalidation generations.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::project::{Change, absolute};

#[derive(Clone)]
enum Input {
    Disk,
    Overlay(Option<Arc<str>>),
}

/// A copy-on-write snapshot of host inputs, independent of thread-local analysis caches.
///
/// Clones retain editor contents and recorded invalidations. Disk contents are acquired
/// by the consuming project; hosts must report filesystem changes before new requests.
#[derive(Clone, Default)]
pub struct Snapshot {
    inputs: Arc<BTreeMap<PathBuf, Arc<Input>>>,
    revision: u64,
}

impl Snapshot {
    /// Returns this input generation.
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the editor contents overriding a path.
    #[must_use]
    pub fn source(&self, path: &Path) -> Option<&Arc<str>> {
        self.overlay(path).and_then(Option::as_ref)
    }

    /// Applies a host event while preserving previously cloned snapshots.
    ///
    /// # Errors
    /// Rejects relative paths.
    pub fn change(&mut self, change: Change) -> io::Result<()> {
        let path = match &change {
            Change::Disk(path) | Change::Close(path) | Change::Overlay { path, .. } => path,
        };

        let path = absolute(path)?;

        let input = match change {
            Change::Disk(_) => self
                .inputs
                .get(&path)
                .map_or(Input::Disk, |input| input.as_ref().clone()),

            Change::Overlay { text, .. } => Input::Overlay(text.map(Arc::from)),
            Change::Close(_) => Input::Disk,
        };

        Arc::make_mut(&mut self.inputs).insert(path, Arc::new(input));
        self.revision += 1;

        Ok(())
    }

    /// Whether consulted inputs retain the same host state as an earlier snapshot.
    #[must_use]
    pub fn current(&self, previous: &Self, inputs: &BTreeSet<PathBuf>) -> bool {
        self.changes(previous).iter().all(|changed| {
            !inputs
                .iter()
                .any(|input| input.starts_with(changed) || changed.starts_with(input))
        })
    }

    pub(crate) fn overlay(&self, path: &Path) -> Option<&Option<Arc<str>>> {
        self.inputs
            .get(path)
            .and_then(|input| match input.as_ref() {
                Input::Disk => None,
                Input::Overlay(text) => Some(text),
            })
    }

    pub(crate) fn sources(&self) -> impl Iterator<Item = &Path> {
        self.inputs
            .iter()
            .filter(|(_, input)| matches!(input.as_ref(), Input::Overlay(Some(_))))
            .map(|(path, _)| path.as_path())
    }

    pub(crate) fn children(&self, path: &Path) -> BTreeSet<PathBuf> {
        self.inputs
            .iter()
            .filter(|(_, input)| matches!(input.as_ref(), Input::Overlay(Some(_))))
            .filter_map(|(file, _)| file.strip_prefix(path).ok()?.components().next())
            .map(|component| path.join(component))
            .collect()
    }

    pub(crate) fn directory(&self, path: &Path) -> bool {
        self.inputs.iter().any(|(file, input)| {
            matches!(input.as_ref(), Input::Overlay(Some(_)))
                && file != path
                && file.starts_with(path)
        })
    }

    pub(crate) fn changes(&self, previous: &Self) -> BTreeSet<PathBuf> {
        self.inputs
            .keys()
            .chain(previous.inputs.keys())
            .filter(
                |path| match (self.inputs.get(*path), previous.inputs.get(*path)) {
                    (Some(current), Some(previous)) => !Arc::ptr_eq(current, previous),
                    _ => true,
                },
            )
            .cloned()
            .collect()
    }
}
