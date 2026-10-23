use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::Arc,
};

use super::Workspace;

/// Immutable dependencies and source snapshots observed by a workspace.
pub struct Observations {
    /// Paths whose changes can affect the observed analysis.
    pub dependencies: BTreeSet<PathBuf>,

    /// Inputs whose changes require rebuilding native state.
    pub reload_dependencies: BTreeSet<PathBuf>,

    /// Source contents associated with observed dependencies.
    pub sources: BTreeMap<String, Arc<str>>,
}

impl Workspace {
    /// Share observations until their dependencies or source snapshots change.
    #[must_use]
    pub fn observations(&self) -> Arc<Observations> {
        let mut cached = self.observations.borrow_mut();

        Arc::clone(cached.get_or_insert_with(|| {
            let dependencies = self.dependencies();

            let sources = dependencies
                .iter()
                .filter_map(|path| {
                    let module = path.to_str()?;

                    Some((module.to_owned(), self.snapshot(module)?))
                })
                .collect();

            Arc::new(Observations {
                dependencies,
                reload_dependencies: self.reload_dependencies(),
                sources,
            })
        }))
    }

    pub(super) fn observe(&self, path: PathBuf) {
        if self.observed_paths.borrow_mut().insert(path) {
            self.observations.borrow_mut().take();
        }
    }
}
