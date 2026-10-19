use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::Arc,
};

use super::{Error, Workspace, paths};

impl Workspace {
    /// Read a source snapshot, retaining it until invalidation or configuration reload.
    ///
    /// # Errors
    /// Returns inaccessible paths or non-UTF-8 source contents.
    pub fn source(&self, module: &str) -> Result<Option<Arc<str>>, Error> {
        self.observed_paths
            .borrow_mut()
            .insert(PathBuf::from(module));

        if let Some(source) = self.snapshots.borrow().get(module) {
            return Ok(source.clone());
        }

        let source = if let Some(source) = self.sources.borrow().get(module) {
            Some(Arc::clone(source))
        } else {
            let path = Path::new(module);

            if !paths::source(path)
                || !paths::metadata(path)?.is_some_and(|metadata| metadata.is_file())
            {
                None
            } else {
                match fs::read_to_string(path) {
                    Ok(source) => Some(Arc::from(source)),

                    Err(error)
                        if matches!(
                            error.kind(),
                            ErrorKind::NotFound | ErrorKind::NotADirectory
                        ) =>
                    {
                        None
                    }

                    Err(source) => {
                        return Err(Error::Filesystem {
                            path: path.to_owned(),
                            source,
                        });
                    }
                }
            }
        };

        self.snapshots
            .borrow_mut()
            .insert(module.to_owned(), source.clone());

        Ok(source)
    }

    /// Return the source snapshot already supplied to an engine.
    #[must_use]
    pub fn snapshot(&self, module: &str) -> Option<Arc<str>> {
        self.snapshots.borrow().get(module).cloned().flatten()
    }

    /// Discard a cached source snapshot while retaining any source overlay.
    pub fn invalidate(&self, module: &str) {
        self.snapshots.borrow_mut().remove(module);
    }

    /// Refresh a source snapshot and report whether its contents changed.
    ///
    /// # Errors
    /// Returns inaccessible paths or non-UTF-8 source contents.
    pub fn refresh(&self, module: &str) -> Result<bool, Error> {
        let previous = self.snapshot(module);
        self.invalidate(module);

        Ok(previous != self.source(module)?)
    }

    /// Replace a source snapshot without writing it to disk.
    ///
    /// New files require an existing parent directory. The snapshot survives reloads.
    ///
    /// # Errors
    /// Returns inaccessible, non-UTF-8, or unsupported source paths.
    pub fn set_source(&self, path: &Path, source: impl Into<Arc<str>>) -> Result<String, Error> {
        let path = paths::identity(&self.root.join(path))?;

        if !paths::source(&path)
            || paths::metadata(&path)?.is_some_and(|metadata| !metadata.is_file())
        {
            return Err(Error::Path {
                path,
                reason: "expected a .luau or .lua source file",
            });
        }

        let module = paths::text(&path)?;
        self.invalidate(&module);

        self.sources
            .borrow_mut()
            .insert(module.clone(), source.into());

        Ok(module)
    }

    /// Remove a source overlay and discard its cached snapshot.
    ///
    /// Returns whether an overlay was present. Subsequent reads use the disk source.
    pub fn remove_source(&self, module: &str) -> bool {
        self.invalidate(module);

        self.sources.borrow_mut().remove(module).is_some()
    }
}
