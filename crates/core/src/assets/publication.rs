use std::{
    collections::BTreeMap,
    fs,
    io::{self, Write as _},
    path::{self, Path, PathBuf},
};

use tempfile::NamedTempFile;

use super::{Error, Generated, generation::interrupted};

pub(super) fn write(
    directory: &Path,
    files: &BTreeMap<PathBuf, String>,
    is_cancelled: &dyn Fn() -> bool,
) -> Result<Generated, Error> {
    let directory = path::absolute(directory).map_err(|source| filesystem(directory, source))?;

    let mut generated = Generated {
        files: Vec::with_capacity(files.len()),
        changed: 0,
    };

    for (relative, contents) in files {
        interrupted(is_cancelled)?;
        let destination = directory.join(relative);

        let parent = destination
            .parent()
            .expect("Internal error: output has no parent directory.");

        let permissions = match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.is_file() => {
                let original = fs::read_to_string(&destination)
                    .map_err(|source| filesystem(&destination, source))?;

                if original == *contents {
                    generated.files.push(destination);
                    continue;
                }

                if metadata.permissions().readonly() {
                    return Err(filesystem(
                        &destination,
                        io::Error::other("Cannot replace a read-only file."),
                    ));
                }

                Some(metadata.permissions())
            }

            Ok(_) => {
                return Err(filesystem(
                    &destination,
                    io::Error::other("Output is not a regular file."),
                ));
            }

            Err(source) if source.kind() == io::ErrorKind::NotFound => None,
            Err(source) => return Err(filesystem(&destination, source)),
        };

        fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;

        let mut staged =
            NamedTempFile::new_in(parent).map_err(|source| filesystem(parent, source))?;

        staged
            .write_all(contents.as_bytes())
            .map_err(|source| filesystem(&destination, source))?;

        if let Some(permissions) = permissions {
            staged
                .as_file()
                .set_permissions(permissions)
                .map_err(|source| filesystem(&destination, source))?;
        }

        staged
            .as_file()
            .sync_all()
            .map_err(|source| filesystem(&destination, source))?;

        staged
            .persist(&destination)
            .map_err(|error| filesystem(&destination, error.error))?;

        generated.files.push(destination);
        generated.changed += 1;
    }

    Ok(generated)
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    Error::Filesystem {
        path: path.to_owned(),
        source,
    }
}
