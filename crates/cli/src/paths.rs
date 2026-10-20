use std::path::{Path, PathBuf};

pub(crate) fn relative(path: &Path, root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();

    for ancestor in root.ancestors() {
        if let Ok(path) = path.strip_prefix(ancestor) {
            return prefix.join(path);
        }

        prefix.push("..");
    }

    path.to_owned()
}
