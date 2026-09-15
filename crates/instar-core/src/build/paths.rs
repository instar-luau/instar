use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};

pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    crate::source::absolute(path).map_err(io::Error::other)
}

pub(super) fn physical(path: &Path) -> io::Result<PathBuf> {
    let path = absolute(path)?;
    let mut missing = Vec::new();
    let mut existing = path.as_path();

    while !existing.exists() {
        missing.push(
            existing
                .file_name()
                .ok_or_else(|| io::Error::other("path has no existing ancestor"))?,
        );

        existing = existing
            .parent()
            .ok_or_else(|| io::Error::other("path has no existing ancestor"))?;
    }

    let mut result = fs::canonicalize(existing)?;

    for part in missing.into_iter().rev() {
        result.push(part);
    }

    Ok(result)
}

pub(super) fn relative(path: &Path) -> io::Result<()> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(io::Error::other(
            "artifact paths must be nonempty relative paths",
        ));
    }

    Ok(())
}

pub(crate) fn safe(root: &Path, relative_path: &Path) -> io::Result<PathBuf> {
    relative(relative_path)?;

    match fs::symlink_metadata(root) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::other(format!(
                "build root is a symlink: {}",
                root.display()
            )));
        }

        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let mut path = root.to_owned();

    for component in relative_path.components() {
        path.push(component);

        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(io::Error::other(format!(
                    "output symlink is unsupported: {}",
                    path.display()
                )));
            }

            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }

    Ok(path)
}

pub(super) fn walk(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_owned()];

    while let Some(path) = pending.pop() {
        let metadata = fs::symlink_metadata(&path)?;

        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "input symlink is unsupported: {}",
                path.display()
            )));
        }

        if metadata.is_file() {
            files.push(absolute(&path)?);
        } else if metadata.is_dir() {
            let mut children = fs::read_dir(path)?
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<io::Result<Vec<_>>>()?;

            children.sort();
            pending.extend(children.into_iter().rev());
        } else {
            return Err(io::Error::other(
                "build inputs must be regular files or directories",
            ));
        }
    }

    files.sort();
    files.dedup();

    Ok(files)
}

pub(super) fn module(path: &Path) -> PathBuf {
    if matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("init.luau" | "init.lua")
    ) {
        path.parent().unwrap_or(path).to_owned()
    } else {
        path.with_extension("")
    }
}

pub(super) fn specifier(from: &Path, to: &Path) -> io::Result<String> {
    let from = module(from);

    let base = from
        .parent()
        .ok_or_else(|| io::Error::other("module has no parent"))?;

    between(base, &module(to))
}

pub(super) fn between(base: &Path, target: &Path) -> io::Result<String> {
    let base = base.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();

    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();

    if common == 0 {
        return Err(io::Error::other(
            "modules occupy different filesystem roots",
        ));
    }

    let mut result = "../".repeat(base.len() - common);

    if result.is_empty() {
        result.push_str("./");
    }

    result.push_str(
        &target[common..]
            .iter()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
    );

    Ok(result)
}
