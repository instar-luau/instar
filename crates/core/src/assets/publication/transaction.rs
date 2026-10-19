use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions, Permissions},
    io::{self, Write as _},
    path::{self, Path, PathBuf},
};

use same_file::Handle;
use tempfile::TempDir;

use crate::{
    assets::{Error, Generated, Target, generation::interrupted},
    workspace,
};

pub(in crate::assets) struct Publication {
    pub(super) originals: BTreeMap<PathBuf, Option<Original>>,
    directory: PathBuf,
    existing: bool,
    _lock: File,
}

#[derive(Eq, PartialEq)]
pub(super) struct Original {
    pub(super) contents: String,
    pub(super) permissions: Permissions,
    pub(super) identity: Handle,
}

pub(super) struct Installed {
    pub(super) relative: PathBuf,
    pub(super) identity: Handle,
    pub(super) permissions: Permissions,
}

impl Publication {
    pub(in crate::assets) fn new(directory: &Path, targets: &[Target]) -> Result<Self, Error> {
        let directory =
            path::absolute(directory).map_err(|source| filesystem(directory, source))?;

        validate_directory(&directory)?;
        let lock = lock(&directory)?;
        validate_directory(&directory)?;
        let existing = exists(&directory)?;
        let mut originals = BTreeMap::new();

        for relative in targets.iter().flat_map(|target| target.files()) {
            let relative = PathBuf::from(relative);
            let destination = directory.join(&relative);

            validate_directory(
                destination
                    .parent()
                    .expect("internal error: output has no parent"),
            )?;

            originals.insert(relative, original(&destination)?);
        }

        Ok(Self {
            originals,
            directory,
            existing,
            _lock: lock,
        })
    }

    pub(in crate::assets) fn contains(&self, path: &Path) -> bool {
        self.originals.contains_key(path)
    }

    pub(in crate::assets) fn write(
        self,
        files: &BTreeMap<PathBuf, String>,
        cancelled: fn() -> bool,
    ) -> Result<Generated, Error> {
        interrupted(cancelled)?;

        if !files.keys().eq(self.originals.keys()) {
            return Err(Error::Invalid {
                reason: "generated files do not match the selected targets".to_owned(),
            });
        }

        self.validate(files)?;

        let changed = self
            .originals
            .iter()
            .filter(|(relative, original)| {
                original.as_ref().map(|original| original.contents.as_str())
                    != Some(files[*relative].as_str())
            })
            .count();

        let generated = Generated {
            files: files
                .keys()
                .map(|relative| self.directory.join(relative))
                .collect(),
            changed,
        };

        if changed == 0 {
            return Ok(generated);
        }

        let parent = self.directory.parent().unwrap_or(&self.directory);
        let staging = tempfile::tempdir_in(parent).map_err(|source| filesystem(parent, source))?;
        let mut replacements = BTreeMap::new();

        for (relative, contents) in files {
            interrupted(cancelled)?;

            if !self.existing {
                stage(&staging.path().join(relative), contents, None)?;
                continue;
            }

            let original = self.originals[relative].as_ref();

            if original.is_some_and(|original| original.contents == *contents) {
                continue;
            }

            let path = if let Some(original) = original {
                let path = staging.path().join("original").join(relative);
                stage(&path, &original.contents, Some(&original.permissions))?;

                path
            } else {
                let path = staging.path().join("proposed").join(relative);
                stage(&path, contents, None)?;

                path
            };

            let replacement = workspace::Replacement::new(&path, contents)
                .map_err(|source| filesystem(&path, source))?;

            let permissions = fs::symlink_metadata(&path)
                .map_err(|source| filesystem(&path, source))?
                .permissions();

            replacements.insert(relative.clone(), (replacement, permissions));
        }

        self.validate(files)?;
        interrupted(cancelled)?;

        if !self.existing {
            fs::rename(staging.path(), &self.directory)
                .map_err(|source| filesystem(&self.directory, source))?;

            return Ok(generated);
        }

        let mut ordered: Vec<_> = self.originals.iter().collect();

        ordered.sort_by_key(|(relative, _)| {
            relative
                .file_name()
                .is_some_and(|name| name == "metadata.json")
        });

        let mut committed = Vec::with_capacity(changed);

        let result = (|| {
            for (relative, original) in ordered {
                let Some((replacement, permissions)) = replacements.remove(relative) else {
                    continue;
                };

                let destination = self.directory.join(relative);

                let parent = destination
                    .parent()
                    .expect("internal error: output has no parent");

                validate_directory(parent)?;
                fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
                validate_directory(parent)?;

                let result = if let Some(original) = original {
                    replacement.commit(&destination, &original.contents, &original.identity)
                } else {
                    replacement.create(&destination)
                };

                let identity = result.map_err(|source| filesystem(&destination, source))?;

                committed.push(Installed {
                    relative: relative.clone(),
                    identity,
                    permissions,
                });
            }

            Ok(())
        })();

        if let Err(source) = result {
            return Err(self.recover(files, staging, &committed, source));
        }

        Ok(generated)
    }

    pub(super) fn recover(
        &self,
        files: &BTreeMap<PathBuf, String>,
        staging: TempDir,
        committed: &[Installed],
        source: Error,
    ) -> Error {
        let mut unrecovered = Vec::new();

        for installed in committed.iter().rev() {
            let destination = self.directory.join(&installed.relative);
            let proposed = &files[&installed.relative];

            let result = (|| {
                validate_directory(
                    destination
                        .parent()
                        .expect("internal error: output has no parent"),
                )?;

                if let Some(original) = &self.originals[&installed.relative] {
                    let backup = staging.path().join("original").join(&installed.relative);

                    workspace::Replacement::new(&backup, &original.contents)
                        .and_then(|replacement| {
                            replacement.commit(&destination, proposed, &installed.identity)
                        })
                        .map_err(|source| filesystem(&destination, source))?;
                } else {
                    workspace::Replacement::remove(
                        &destination,
                        proposed,
                        &installed.permissions,
                        &installed.identity,
                    )
                    .map_err(|source| filesystem(&destination, source))?;
                }

                Ok::<(), Error>(())
            })();

            if result.is_err() {
                unrecovered.push(destination);
            }
        }

        if unrecovered.is_empty() {
            source
        } else {
            Error::Recovery {
                source: Box::new(source),
                backup: staging.keep(),
                unrecovered,
            }
        }
    }

    fn validate(&self, files: &BTreeMap<PathBuf, String>) -> Result<(), Error> {
        validate_directory(&self.directory)?;

        if exists(&self.directory)? != self.existing {
            return Err(filesystem(
                &self.directory,
                io::Error::other("output directory changed during generation"),
            ));
        }

        for (relative, expected) in &self.originals {
            let path = self.directory.join(relative);
            validate_directory(path.parent().expect("internal error: output has no parent"))?;

            if original(&path)? != *expected {
                return Err(filesystem(
                    &path,
                    io::Error::other("output changed during generation"),
                ));
            }

            if expected
                .as_ref()
                .is_some_and(|original| original.contents != files[relative])
                && fs::symlink_metadata(&path)
                    .map_err(|source| filesystem(&path, source))?
                    .permissions()
                    .readonly()
            {
                return Err(filesystem(
                    &path,
                    io::Error::other("refusing to replace a read-only file"),
                ));
            }
        }

        Ok(())
    }
}

fn validate_directory(path: &Path) -> Result<(), Error> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(metadata) if metadata.is_dir() => {}

            Ok(_) => {
                return Err(filesystem(
                    ancestor,
                    io::Error::other("output parent is not a regular directory"),
                ));
            }

            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => return Err(filesystem(ancestor, source)),
        }
    }

    Ok(())
}

fn original(path: &Path) -> Result<Option<Original>, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let mut file = File::open(path).map_err(|source| filesystem(path, source))?;

            let contents =
                io::read_to_string(&mut file).map_err(|source| filesystem(path, source))?;

            let permissions = metadata.permissions();
            let identity = Handle::from_file(file).map_err(|source| filesystem(path, source))?;
            let current = fs::symlink_metadata(path).map_err(|source| filesystem(path, source))?;

            if !current.is_file()
                || current.permissions() != permissions
                || identity != Handle::from_path(path).map_err(|source| filesystem(path, source))?
            {
                return Err(filesystem(
                    path,
                    io::Error::other("output changed during generation"),
                ));
            }

            Ok(Some(Original {
                contents,
                permissions,
                identity,
            }))
        }

        Ok(_) => Err(filesystem(
            path,
            io::Error::other("output is not a regular file"),
        )),

        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(filesystem(path, source)),
    }
}

fn exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(filesystem(path, source)),
    }
}

fn lock(directory: &Path) -> Result<File, Error> {
    let parent = directory.parent().unwrap_or(directory);
    fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
    validate_directory(parent)?;

    let directory = if exists(directory)? {
        fs::canonicalize(directory).map_err(|source| filesystem(directory, source))?
    } else {
        fs::canonicalize(parent)
            .map_err(|source| filesystem(parent, source))?
            .join(directory.file_name().unwrap_or_default())
    };

    let parent = directory.parent().unwrap_or(&directory);

    let mut name = directory.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    let path = parent.join(name);

    if exists(&path)?
        && !fs::symlink_metadata(&path)
            .map_err(|source| filesystem(&path, source))?
            .is_file()
    {
        return Err(filesystem(
            &path,
            io::Error::other("publication lock is not a regular file"),
        ));
    }

    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|source| filesystem(&path, source))?;

    file.lock().map_err(|source| filesystem(&path, source))?;
    let metadata = fs::symlink_metadata(&path).map_err(|source| filesystem(&path, source))?;

    if !metadata.is_file()
        || Handle::from_file(
            file.try_clone()
                .map_err(|source| filesystem(&path, source))?,
        )
        .map_err(|source| filesystem(&path, source))?
            != Handle::from_path(&path).map_err(|source| filesystem(&path, source))?
    {
        return Err(filesystem(
            &path,
            io::Error::other("publication lock changed before acquisition"),
        ));
    }

    Ok(file)
}

pub(super) fn stage(
    path: &Path,
    contents: &str,
    permissions: Option<&Permissions>,
) -> Result<(), Error> {
    let parent = path.parent().expect("internal error: output has no parent");
    fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
    let mut file = File::create(path).map_err(|source| filesystem(path, source))?;

    file.write_all(contents.as_bytes())
        .map_err(|source| filesystem(path, source))?;

    if let Some(permissions) = permissions {
        file.set_permissions(permissions.clone())
            .map_err(|source| filesystem(path, source))?;
    }

    file.sync_all().map_err(|source| filesystem(path, source))
}

pub(super) fn filesystem(path: &Path, source: io::Error) -> Error {
    Error::Filesystem {
        path: path.to_owned(),
        source,
    }
}
