use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

fn configuration(path: &Path) -> io::Result<PathBuf> {
    crate::project::nearest_configuration(path)
        .or_else(|| path.parent().map(Path::to_owned))
        .ok_or_else(|| io::Error::other("source has no parent"))
}

pub(super) fn files(roots: &BTreeSet<PathBuf>) -> io::Result<Vec<PathBuf>> {
    let mut pending = roots
        .iter()
        .cloned()
        .map(|path| (path, true))
        .collect::<Vec<_>>();

    let mut directories = BTreeSet::new();
    let mut files = BTreeSet::new();
    let mut configurations = BTreeMap::new();

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
        } else if metadata.is_file() {
            if matches!(
                path.file_name().and_then(|name| name.to_str()),
                Some("instar.toml" | ".luaurc" | ".config.luau" | "config.luau")
            ) {
                continue;
            }

            let selected = if explicit {
                true
            } else {
                let configuration_path = configuration(&path)?;

                if !configurations.contains_key(&configuration_path) {
                    configurations.insert(
                        configuration_path.clone(),
                        (
                            crate::project::Configuration::discover_frontends(&path)?,
                            crate::project::selection::Selection::discover(
                                &path,
                                crate::project::selection::Scope::Analyze,
                            )?,
                        ),
                    );
                }

                let (configuration, selection) = configurations
                    .get(&configuration_path)
                    .expect("configuration inserted");

                configuration.language(&path) && selection.includes(&path)?
            };

            if selected {
                files.insert(crate::source::absolute(&path).map_err(io::Error::other)?);
            }
        }
    }

    Ok(files.into_iter().collect())
}
