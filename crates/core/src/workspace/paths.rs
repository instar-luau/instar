use std::{
    fs::{self, Metadata},
    io::ErrorKind,
    path::{MAIN_SEPARATOR, Path, PathBuf},
};

use super::Error;

pub(super) fn canonical(path: &Path) -> Result<PathBuf, Error> {
    let canonical = fs::canonicalize(path).map_err(|source| Error::Filesystem {
        path: path.to_owned(),
        source,
    })?;

    Ok(PathBuf::from(text(&canonical)?))
}

pub(super) fn identity(path: &Path) -> Result<PathBuf, Error> {
    if metadata(path)?.is_some() {
        return canonical(path);
    }

    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(Error::Path {
            path: path.to_owned(),
            reason: "Expected a source file path.",
        });
    };

    Ok(directory(parent)?.join(name))
}

pub(super) fn read(path: &Path) -> Result<String, Error> {
    fs::read_to_string(path).map_err(|source| Error::Filesystem {
        path: path.to_owned(),
        source,
    })
}

pub(super) fn text(path: &Path) -> Result<String, Error> {
    let text = path.to_str().ok_or_else(|| Error::Path {
        path: path.to_owned(),
        reason: "Module paths must be valid UTF-8.",
    })?;

    #[cfg(windows)]
    if let Some(network) = text.strip_prefix(r"\\?\UNC\") {
        return Ok(format!("//{}", network.replace(MAIN_SEPARATOR, "/")));
    }

    #[cfg(windows)]
    let text = text.strip_prefix(r"\\?\").unwrap_or(text);

    Ok(text.replace(MAIN_SEPARATOR, "/"))
}

pub(super) fn metadata(path: &Path) -> Result<Option<Metadata>, Error> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),

        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(None)
        }

        Err(source) => Err(Error::Filesystem {
            path: path.to_owned(),
            source,
        }),
    }
}

pub(super) fn directory(path: &Path) -> Result<PathBuf, Error> {
    let canonical = canonical(path)?;

    if !metadata(&canonical)?.is_some_and(|metadata| metadata.is_dir()) {
        return Err(Error::Path {
            path: canonical,
            reason: "Expected a directory.",
        });
    }

    Ok(canonical)
}

pub(super) fn is_source(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "luau" || extension == "lua")
}
