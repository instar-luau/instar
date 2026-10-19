use std::{
    fs::{self, OpenOptions, Permissions},
    io::{self, Read as _, Write as _},
    path::Path,
};

use same_file::Handle;
use tempfile::NamedTempFile;

use super::{Error, Workspace};

impl Workspace {
    /// Atomically replace a disk source if its original bytes remain current.
    ///
    /// Preserves permissions and refuses overlays, links, and read-only files.
    ///
    /// # Errors
    /// Returns invalid destinations, concurrent changes, locking, or filesystem failures.
    pub fn write(&self, module: &str, original: &str, proposed: &str) -> Result<(), Error> {
        let path = Path::new(module);

        if self.sources.contains_key(module) || self.module(path)? != module {
            return Err(Error::Path {
                path: path.to_owned(),
                reason: "source is not an authorized disk destination",
            });
        }

        if original == proposed {
            return Ok(());
        }

        replace(path, original, proposed).map_err(|source| Error::Filesystem {
            path: path.to_owned(),
            source,
        })?;

        self.invalidate(module);

        Ok(())
    }
}

/// Atomically replace a regular, writable file while preserving its permissions.
///
/// # Errors
/// Returns concurrent changes, locking failures, or filesystem errors.
pub fn replace(path: &Path, original: &str, proposed: &str) -> io::Result<()> {
    guarded(path, original, |permissions| {
        Replacement::prepare(path, proposed, permissions)
    })?;

    Ok(())
}

pub(crate) struct Replacement {
    staged: NamedTempFile,
    permissions: Permissions,
}

impl Replacement {
    pub(crate) fn new(path: &Path, proposed: &str) -> io::Result<Self> {
        Self::prepare(path, proposed, fs::symlink_metadata(path)?.permissions())
    }

    pub(crate) fn commit(self, path: &Path, original: &str) -> io::Result<()> {
        guarded(path, original, |_| Ok(self))?;

        Ok(())
    }

    fn prepare(path: &Path, proposed: &str, permissions: Permissions) -> io::Result<Self> {
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("write destination has no parent"))?;

        let mut staged = NamedTempFile::new_in(parent)?;
        staged.write_all(proposed.as_bytes())?;
        staged.as_file().set_permissions(permissions.clone())?;
        staged.as_file().sync_all()?;

        Ok(Self {
            staged,
            permissions,
        })
    }
}

fn guarded(
    path: &Path,
    original: &str,
    prepare: impl FnOnce(Permissions) -> io::Result<Replacement>,
) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;

    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(io::Error::other(
            "refusing to replace a nonregular or read-only file",
        ));
    }

    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    file.try_lock().map_err(io::Error::other)?;

    if !file.metadata()?.is_file() {
        return Err(io::Error::other("refusing to replace a nonregular file"));
    }

    let Replacement {
        mut staged,
        permissions,
    } = prepare(metadata.permissions())?;

    if permissions != metadata.permissions() {
        return Err(io::Error::other(
            "write destination changed before replacement",
        ));
    }

    let mut current = Vec::new();

    (&mut file)
        .take(
            u64::try_from(original.len())
                .map_err(io::Error::other)?
                .saturating_add(1),
        )
        .read_to_end(&mut current)?;

    if current != original.as_bytes() {
        return Err(io::Error::other("file changed before replacement"));
    }

    let current_metadata = fs::symlink_metadata(path)?;

    if !current_metadata.is_file() || current_metadata.permissions() != metadata.permissions() {
        return Err(io::Error::other(
            "write destination changed before replacement",
        ));
    }

    if Handle::from_file(file.try_clone()?)? != Handle::from_path(path)? {
        return Err(io::Error::other(
            "write destination changed before replacement",
        ));
    }

    fs::rename(staged.path(), path)?;
    staged.disable_cleanup(true);

    Ok(())
}
