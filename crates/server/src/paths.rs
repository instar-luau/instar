use std::path::{Component, Path, PathBuf};

use tower_lsp_server::jsonrpc::{Error, Result};

use crate::error::failure;

pub(crate) fn is_intersecting(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

pub(crate) fn destination(path: &Path) -> Result<PathBuf> {
    let path = lexical(path)?;

    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(Error::invalid_params(
            "File rename destination has no name.",
        ));
    };

    Ok(identity(parent)?.join(name))
}

pub(crate) fn identity(path: &Path) -> Result<PathBuf> {
    let normalized = lexical(path)?;
    let mut ancestor = normalized.as_path();
    let mut remainder = Vec::new();

    while !ancestor.try_exists().map_err(failure)? {
        let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) else {
            return Err(Error::invalid_params(
                "File operation path has no existing ancestor.",
            ));
        };

        remainder.push(name);
        ancestor = parent;
    }

    let mut canonical = ancestor.canonicalize().map_err(failure)?;

    for component in remainder.into_iter().rev() {
        canonical.push(component);
    }

    lexical(&canonical)
}

pub(crate) fn lexical(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.to_str().is_none_or(|path| path.contains('\0')) {
        return Err(Error::invalid_params(
            "File operation paths must be absolute UTF-8 paths without null characters.",
        ));
    }

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

    let text = normalized
        .to_str()
        .ok_or_else(|| Error::invalid_params("File operation path is not UTF-8."))?;

    #[cfg(windows)]
    let text = if let Some(network) = text.strip_prefix(r"\\?\UNC\") {
        format!("//{}", network.replace('\\', "/"))
    } else {
        text.strip_prefix(r"\\?\")
            .unwrap_or(text)
            .replace('\\', "/")
    };

    Ok(PathBuf::from(text))
}
