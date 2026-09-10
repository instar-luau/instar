use std::{
    collections::BTreeSet,
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::Args;
use instar_core::{analysis, resolution::Resolver, source::SourceStore};

#[derive(Args)]
pub struct Analyze {
    /// Files or directories to analyze; '-' reads original bytes from stdin.
    #[arg(required = true)]
    files: Vec<PathBuf>,

    /// Module filename for stdin, used for relative imports and configuration.
    #[arg(long)]
    filename: Option<PathBuf>,

    /// Override the default checking mode for files without a mode directive.
    #[arg(long, value_parser = ["strict"])]
    mode: Option<String>,

    /// Select the upstream type solver.
    #[arg(long, value_parser = ["new", "old"])]
    solver: Option<String>,

    /// Emit source with inferred type annotations.
    #[arg(long)]
    annotate: bool,
}

impl Analyze {
    pub fn run(self) -> io::Result<ExitCode> {
        let mut sources = SourceStore::default();
        let mut modules = Vec::new();
        let mut directories = BTreeSet::new();
        let mut pending = self.files;
        let mut stdin = None;
        if self.filename.is_some() && !pending.iter().any(|path| path == Path::new("-")) {
            return Err(io::Error::other("--filename requires '-' input"));
        }
        while let Some(path) = pending.pop() {
            if path == Path::new("-") {
                if stdin.is_none() {
                    let mut bytes = Vec::new();
                    io::stdin().lock().read_to_end(&mut bytes)?;
                    // Luau's VfsNavigator names unnamed stdin 'stdin' under cwd.
                    let path = self
                        .filename
                        .clone()
                        .unwrap_or_else(|| PathBuf::from("stdin"));
                    let source = sources
                        .open_bytes(&path, 0, bytes)
                        .map_err(io::Error::other)?;
                    modules.push(source.path().to_owned());
                    stdin = Some(source);
                }
                continue;
            }
            let metadata = fs::metadata(&path).map_err(|error| {
                io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?;
            if metadata.is_file() {
                let source = sources.read(&path).map_err(io::Error::other)?;
                modules.push(source.path().to_owned());
            } else if metadata.is_dir() {
                if !directories.insert(fs::canonicalize(&path)?) {
                    continue;
                }
                let mut children = Vec::new();
                for entry in fs::read_dir(&path)? {
                    let entry = entry?;
                    let path = entry.path();
                    let metadata = fs::metadata(&path).map_err(|error| {
                        io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                    })?;
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
                pending.extend(children.into_iter().rev());
            } else {
                return Err(io::Error::other(format!(
                    "{}: expected a file or directory",
                    path.display()
                )));
            }
        }
        let options = analysis::Options {
            strict: self.mode.is_some(),
            old_solver: self.solver.as_deref() == Some("old"),
            annotations: self.annotate,
        };
        let report = analysis::analyze(&mut Resolver::new(&mut sources), &modules, &options)?;
        for diagnostic in &report.diagnostics {
            eprintln!(
                "{}({},{}): {}",
                diagnostic.path.display(),
                diagnostic.line + 1,
                diagnostic.column + 1,
                diagnostic.message
            );
        }
        let mut output = io::stdout().lock();
        for annotation in &report.annotations {
            output.write_all(&annotation.bytes)?;
        }
        Ok(if report.has_errors() {
            ExitCode::FAILURE
        } else {
            ExitCode::SUCCESS
        })
    }
}
