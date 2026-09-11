use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    sync::Arc,
};

use clap::Args;
use instar_core::source::{Source, SourceStore};

#[derive(Args)]
pub struct Input {
    /// Files or directories to process; '-' reads original bytes from stdin.
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Module filename for stdin, used for relative imports and configuration.
    #[arg(long)]
    filename: Option<PathBuf>,
}

pub struct Loaded {
    pub store: SourceStore,
    pub sources: Vec<Arc<Source>>,
    pub standard_input: Option<PathBuf>,
}

impl Input {
    pub fn load_selected(
        self,
        mut selected: impl FnMut(&Path) -> io::Result<bool>,
    ) -> io::Result<Loaded> {
        if self.filename.is_some() && !self.files.iter().any(|path| path == Path::new("-")) {
            return Err(io::Error::other("--filename requires '-' input"));
        }

        let mut store = SourceStore::default();
        let mut paths = Vec::new();
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
                paths.push(source.path().to_owned());
            } else if metadata.is_dir() {
                if !directories.insert(fs::canonicalize(&path)?) {
                    continue;
                }

                let mut children = Vec::new();

                for entry in fs::read_dir(&path)? {
                    let path = entry?.path();
                    let metadata = inspect(&path)?;

                    if metadata.is_dir()
                        || matches!(
                            path.extension().and_then(|extension| extension.to_str()),
                            Some("lua" | "luau")
                        )
                    {
                        children.push(path);
                    }
                }

                children.sort();
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
            .map(|path| store.read(&path).map_err(io::Error::other))
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
