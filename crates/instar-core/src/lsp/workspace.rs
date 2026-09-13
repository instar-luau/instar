use std::{collections::BTreeSet, fs, io, path::PathBuf};

pub(super) fn files(roots: &BTreeSet<PathBuf>) -> io::Result<Vec<PathBuf>> {
    let mut pending = roots
        .iter()
        .cloned()
        .map(|path| (path, true))
        .collect::<Vec<_>>();

    let mut directories = BTreeSet::new();
    let mut files = BTreeSet::new();

    while let Some((path, explicit)) = pending.pop() {
        let metadata = fs::metadata(&path)?;

        if metadata.is_dir() {
            if !directories.insert(fs::canonicalize(&path)?) {
                continue;
            }

            pending.extend(
                crate::source::children(&path)?
                    .into_iter()
                    .rev()
                    .map(|path| (path, false)),
            );
        } else if metadata.is_file()
            && (explicit
                || crate::project::selection::Selection::discover(&path)?.includes(&path)?)
        {
            files.insert(crate::source::absolute(&path).map_err(io::Error::other)?);
        }
    }

    Ok(files.into_iter().collect())
}
