use super::EditorEntry;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub(super) struct File {
    pub symbols: Vec<EditorEntry>,
    pub calls: Vec<EditorEntry>,
    pub dependencies: BTreeSet<PathBuf>,
}

#[derive(Default)]
pub(super) struct Index {
    pub files: BTreeMap<PathBuf, File>,
}

impl Index {
    pub fn invalidate(&mut self, path: &Path) {
        let mut changed = BTreeSet::from([path.to_owned()]);

        loop {
            let dependents = self
                .files
                .iter()
                .filter(|(_, file)| !file.dependencies.is_disjoint(&changed))
                .map(|(path, _)| path.clone())
                .collect::<Vec<_>>();

            let previous = changed.len();
            changed.extend(dependents);

            if previous == changed.len() {
                break;
            }
        }

        self.files.retain(|path, _| !changed.contains(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalidation_follows_dependencies_and_retains_unrelated_files() {
        let mut index = Index::default();

        for (name, dependency) in [
            ("first", "second"),
            ("second", "third"),
            ("third", "first"),
            ("unrelated", "outside"),
        ] {
            index.files.insert(
                name.into(),
                File {
                    symbols: Vec::new(),
                    calls: Vec::new(),
                    dependencies: BTreeSet::from([dependency.into()]),
                },
            );
        }

        index.invalidate(Path::new("third"));

        assert_eq!(
            index.files.keys().cloned().collect::<Vec<_>>(),
            [PathBuf::from("unrelated")]
        );
    }
}
