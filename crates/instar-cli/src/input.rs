use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Args;
use instar_core::source::{Source, SourceStore};

#[derive(Default, Args)]
pub(super) struct Input {
    /// Files or directories to process; '-' reads original bytes from stdin.
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Module filename for stdin, used for relative imports and configuration.
    #[arg(long)]
    filename: Option<PathBuf>,
}

pub(super) struct Loaded {
    pub(super) store: SourceStore,
    pub(super) sources: Vec<Arc<Source>>,
    pub(super) standard_input: Option<PathBuf>,
}

impl Input {
    pub(super) fn new(files: Vec<PathBuf>, filename: Option<PathBuf>) -> Self {
        Self { files, filename }
    }

    pub(super) fn load_selected(
        self,
        mut selected: impl FnMut(&Path) -> io::Result<bool>,
    ) -> io::Result<Loaded> {
        if self.filename.is_some() && !self.files.iter().any(|path| path == Path::new("-")) {
            return Err(io::Error::other("--filename requires '-' input"));
        }

        let mut store = SourceStore::default();
        let mut paths = Vec::new();
        let mut loaded = BTreeMap::new();
        let mut directories = BTreeSet::new();
        let mut pending: Vec<_> = self.files.into_iter().map(|path| (path, true)).collect();
        let mut standard_input = None;

        while let Some((path, explicit)) = pending.pop() {
            if path == Path::new("-") {
                if standard_input.is_none() {
                    let mut bytes = Vec::new();
                    io::stdin().lock().read_to_end(&mut bytes)?;

                    let path = self
                        .filename
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("stdin"));

                    let source = store
                        .open_bytes(&path, 0, bytes)
                        .map_err(io::Error::other)?;

                    loaded.insert(source.path().to_owned(), Arc::clone(&source));
                    paths.push(source.path().to_owned());
                    standard_input = Some(source.path().to_owned());
                }

                continue;
            }

            let metadata = inspect(&path)?;

            if metadata.is_file() {
                if !explicit && !selected(&path)? {
                    continue;
                }

                let source = store.read(&path).map_err(io::Error::other)?;
                loaded.insert(source.path().to_owned(), Arc::clone(&source));
                paths.push(source.path().to_owned());
            } else if metadata.is_dir() {
                if !directories.insert(fs::canonicalize(&path)?) {
                    continue;
                }

                let children = instar_core::source::children(&path)?;
                pending.extend(children.into_iter().rev().map(|path| (path, false)));
            } else {
                return Err(io::Error::other(format!(
                    "{}: expected a file or directory",
                    path.display()
                )));
            }
        }

        let mut seen = BTreeSet::new();

        let sources = paths
            .into_iter()
            .filter(|path| seen.insert(path.clone()))
            .map(|path| {
                loaded
                    .remove(&path)
                    .ok_or_else(|| io::Error::other("input source was not loaded"))
            })
            .collect::<io::Result<Vec<_>>>()?;

        Ok(Loaded {
            store,
            sources,
            standard_input,
        })
    }
}

fn inspect(path: &Path) -> io::Result<fs::Metadata> {
    fs::metadata(path)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}
