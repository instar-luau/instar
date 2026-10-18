use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write as _},
    path::{self, Path, PathBuf},
};

use super::{Error, Generated, Target, generation::interrupted};
use crate::workspace;

pub(super) struct Publication {
    directory: PathBuf,
    existing: bool,
    originals: BTreeMap<PathBuf, Option<String>>,
}

impl Publication {
    pub(super) fn new(directory: &Path, targets: &[Target]) -> Result<Self, Error> {
        let directory =
            path::absolute(directory).map_err(|source| filesystem(directory, source))?;

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
            directory,
            existing,
            originals,
        })
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        self.originals.contains_key(path)
    }

    pub(super) fn write(
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
            .filter(|(relative, original)| original.as_deref() != Some(files[*relative].as_str()))
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
        fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
        validate_directory(parent)?;
        let staging = tempfile::tempdir_in(parent).map_err(|source| filesystem(parent, source))?;

        for (relative, contents) in files {
            interrupted(cancelled)?;
            let path = staging.path().join(relative);
            let parent = path.parent().expect("internal error: output has no parent");
            fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
            let mut file = File::create(&path).map_err(|source| filesystem(&path, source))?;

            file.write_all(contents.as_bytes())
                .map_err(|source| filesystem(&path, source))?;

            file.sync_all()
                .map_err(|source| filesystem(&path, source))?;
        }

        interrupted(cancelled)?;
        self.validate(files)?;

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

        for (relative, original) in ordered {
            interrupted(cancelled)?;
            let proposed = &files[relative];

            if original.as_deref() == Some(proposed.as_str()) {
                continue;
            }

            let destination = self.directory.join(relative);

            let parent = destination
                .parent()
                .expect("internal error: output has no parent");

            validate_directory(parent)?;
            fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;
            validate_directory(parent)?;

            if let Some(original) = original {
                workspace::replace(&destination, original, proposed)
                    .map_err(|source| filesystem(&destination, source))?;
            } else {
                let mut file = tempfile::NamedTempFile::new_in(parent)
                    .map_err(|source| filesystem(parent, source))?;

                file.write_all(proposed.as_bytes())
                    .map_err(|source| filesystem(&destination, source))?;

                let permissions = fs::metadata(staging.path().join(relative))
                    .map_err(|source| filesystem(&destination, source))?
                    .permissions();

                file.as_file()
                    .set_permissions(permissions)
                    .map_err(|source| filesystem(&destination, source))?;

                file.as_file()
                    .sync_all()
                    .map_err(|source| filesystem(&destination, source))?;

                file.persist_noclobber(&destination)
                    .map_err(|error| filesystem(&destination, error.error))?;
            }
        }

        Ok(generated)
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
                .as_deref()
                .is_some_and(|original| original != files[relative])
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

fn original(path: &Path) -> Result<Option<String>, Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => fs::read_to_string(path)
            .map(Some)
            .map_err(|source| filesystem(path, source)),

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

fn filesystem(path: &Path, source: io::Error) -> Error {
    Error::Filesystem {
        path: path.to_owned(),
        source,
    }
}
