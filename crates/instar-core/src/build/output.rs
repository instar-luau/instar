use super::{Plan, paths};
use serde::{Deserialize, Serialize};

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    files: BTreeSet<PathBuf>,
}

#[derive(Debug, Default, Serialize)]
/// Artifact counts after publishing a build plan.
pub struct Outcome {
    /// Artifacts written with new contents.
    pub written: usize,

    /// Artifacts whose existing contents already matched.
    pub unchanged: usize,

    /// Previously owned artifacts pruned from the output.
    pub removed: usize,
}

struct Lock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl Drop for Lock {
    fn drop(&mut self) {
        drop(self.file.take());

        if let Err(error) = fs::remove_file(&self.path) {
            eprintln!("{}: {error}", self.path.display());
        }
    }
}

fn write(path: &Path, bytes: &[u8], replace: bool) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("output has no parent"))?;

    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;

    if replace {
        file.persist(path)
    } else {
        file.persist_noclobber(path)
    }
    .map_err(|error| error.error)?;

    Ok(())
}

pub(super) fn publish(plan: &Plan) -> io::Result<Outcome> {
    if plan.has_errors() {
        return Err(io::Error::other(
            "build contains errors; previous output retained",
        ));
    }

    let observed = super::observe(&plan.root, plan.profile.as_deref(), &plan.inputs())?;

    if let Some(path) = observed
        .keys()
        .chain(plan.snapshots.keys())
        .find(|path| observed.get(path.as_path()) != plan.snapshots.get(path.as_path()))
    {
        return Err(io::Error::other(format!(
            "build input changed before publication: {}",
            path.display()
        )));
    }

    let state = paths::safe(&plan.directory, &plan.state)?;
    fs::create_dir_all(&state)?;

    for name in ["lock", "manifest.json", "files"] {
        paths::safe(&state, Path::new(name))?;
    }

    let path = state.join("lock");

    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("{}: build lock unavailable: {error}", path.display()),
            )
        })?;

    let _lock = Lock {
        path,
        file: Some(file),
    };

    let originals = ownership(plan, &state)?;
    let prepared = tempfile::tempdir_in(&state)?;

    for (path, bytes) in &plan.prepared {
        write(&paths::safe(prepared.path(), path)?, bytes, false)?;
    }

    let mut outcome = Outcome::default();
    let mut changed = Vec::new();

    let result = commit(plan, &originals, &mut outcome, &mut changed)
        .and_then(|()| record(&state, &prepared, plan));

    if let Err(error) = result {
        let mut failures = Vec::new();

        for path in changed.iter().rev() {
            let restored = paths::safe(&plan.directory, path).and_then(|destination| {
                if let Some(bytes) = originals.get(path) {
                    write(&destination, bytes, true)
                } else {
                    match fs::remove_file(&destination) {
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        result => result,
                    }
                }
            });

            if let Err(error) = restored {
                failures.push(format!("{}: {error}", path.display()));
            }
        }

        return Err(io::Error::new(
            error.kind(),
            format!(
                "build publication failed: {error}; rollback failures: {}",
                failures.join("; ")
            ),
        ));
    }

    Ok(outcome)
}

fn ownership(plan: &Plan, state: &Path) -> io::Result<BTreeMap<PathBuf, Vec<u8>>> {
    let previous = match fs::read(state.join("manifest.json")) {
        Ok(bytes) => {
            let manifest: Manifest = serde_json::from_slice(&bytes).map_err(io::Error::other)?;

            if manifest.version != 1 {
                return Err(io::Error::other("unsupported build manifest version"));
            }

            manifest.files
        }

        Err(error) if error.kind() == io::ErrorKind::NotFound => BTreeSet::new(),
        Err(error) => return Err(error),
    };

    let baseline = state.join("files");
    let mut originals = BTreeMap::new();

    for path in previous.union(&plan.prepared.keys().cloned().collect()) {
        if path.starts_with(".instar") {
            return Err(io::Error::other("artifact overlaps build state"));
        }

        let destination = paths::safe(&plan.directory, path)?;

        if previous.contains(path) {
            let expected = fs::read(paths::safe(&baseline, path)?)?;
            let actual = fs::read(&destination)?;

            if expected != actual {
                return Err(io::Error::other(format!(
                    "owned output changed outside the builder: {}",
                    destination.display()
                )));
            }

            originals.insert(path.clone(), expected);
        } else if destination.exists() {
            return Err(io::Error::other(format!(
                "refusing to overwrite unowned output: {}",
                destination.display()
            )));
        }
    }

    Ok(originals)
}

fn record(state: &Path, prepared: &tempfile::TempDir, plan: &Plan) -> io::Result<()> {
    let manifest = Manifest {
        version: 1,
        files: plan.prepared.keys().cloned().collect(),
    };

    let bytes = serde_json::to_vec(&manifest).map_err(io::Error::other)?;
    let backup = tempfile::tempdir_in(state)?;
    let original = backup.path().join("files");
    let baseline = state.join("files");

    if baseline.exists() {
        fs::rename(&baseline, &original)?;
    }

    if let Err(error) = fs::rename(prepared.path(), &baseline) {
        if original.exists()
            && let Err(rollback) = fs::rename(&original, &baseline)
        {
            return Err(retain(backup, &error, &rollback));
        }

        return Err(error);
    }

    if let Err(error) = write(&state.join("manifest.json"), &bytes, true) {
        if let Err(rollback) = fs::rename(&baseline, prepared.path()) {
            return Err(retain(backup, &error, &rollback));
        }

        if original.exists()
            && let Err(rollback) = fs::rename(&original, &baseline)
        {
            return Err(retain(backup, &error, &rollback));
        }

        return Err(error);
    }

    Ok(())
}

fn retain(backup: tempfile::TempDir, error: &io::Error, rollback: &io::Error) -> io::Error {
    let path = backup.keep();

    io::Error::new(
        error.kind(),
        format!(
            "{error}; snapshot rollback failed: {rollback}; previous snapshot retained at {}",
            path.display()
        ),
    )
}

fn commit(
    plan: &Plan,
    previous: &BTreeMap<PathBuf, Vec<u8>>,
    outcome: &mut Outcome,
    changed: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for (path, bytes) in &plan.prepared {
        if previous.get(path) == Some(bytes) {
            outcome.unchanged += 1;
            continue;
        }

        let destination = paths::safe(&plan.directory, path)?;

        if let Some(expected) = previous.get(path)
            && fs::read(&destination)? != *expected
        {
            return Err(io::Error::other(format!(
                "owned output changed before replacement: {}",
                destination.display()
            )));
        }

        write(&destination, bytes, previous.contains_key(path))?;
        changed.push(path.clone());
        outcome.written += 1;
    }

    for (path, expected) in previous {
        if !plan.prepared.contains_key(path) {
            let destination = paths::safe(&plan.directory, path)?;

            if fs::read(&destination)? != *expected {
                return Err(io::Error::other(format!(
                    "owned output changed before pruning: {}",
                    destination.display()
                )));
            }

            fs::remove_file(destination)?;
            changed.push(path.clone());
            outcome.removed += 1;
        }
    }

    Ok(())
}
