//! Source discovery respecting filesystem ignore rules and editor selection.

use std::{
    collections::BTreeSet,
    fs, io, iter,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Options, selection};

use crate::{
    configuration::Scope,
    project::{Project, absolute},
    source::Kind,
};

impl Project {
    /// Discovers filesystem and open overlay sources selected for editor indexing.
    ///
    /// # Errors
    /// Returns invalid roots, filesystem, configuration, selection, or interruption failures.
    pub fn index_sources(
        &mut self,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<PathBuf>> {
        let started = Instant::now();

        self.during(
            |project| {
                let mut paths = BTreeSet::new();

                for root in roots {
                    project.interrupted()?;
                    let root = absolute(root)?;
                    project.view.consult(iter::once(&root));

                    match fs::metadata(&root) {
                        Ok(_) => paths.extend(files(&root, &options.remaining(started))?),

                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                            ) => {}

                        Err(error) => return Err(error),
                    }

                    for path in project
                        .view
                        .snapshot
                        .sources()
                        .filter(|path| path.starts_with(&root))
                    {
                        project.interrupted()?;
                        paths.insert(path.to_path_buf());
                    }
                }

                let mut selected = Vec::new();

                for path in paths {
                    project.interrupted()?;

                    if !matches!(
                        path.extension().and_then(|extension| extension.to_str()),
                        Some("lua" | "luau")
                    ) || project.view.kind(&path)? != Some(Kind::File)
                    {
                        continue;
                    }

                    let settings = project.configuration(
                        path.parent()
                            .ok_or_else(|| io::Error::other("source has no parent directory"))?,
                    )?;

                    let index = &settings.configuration.editor.index;

                    if selection::matches(&path, index.include.as_deref(), true)?
                        && !selection::matches(&path, index.exclude.as_deref(), false)?
                    {
                        selected.push(path);
                    }
                }

                Ok(selected)
            },
            options,
        )
    }

    /// Discovers workspace diagnostic sources selected by effective editor settings.
    ///
    /// # Errors
    /// Returns filesystem, configuration, selection, or interruption failures.
    pub fn diagnostic_sources(
        &mut self,
        roots: &[PathBuf],
        options: &Options,
    ) -> io::Result<Vec<PathBuf>> {
        let started = Instant::now();

        self.during(
            |project| {
                let mut selected = Vec::new();

                for path in project.index_sources(roots, &options.remaining(started))? {
                    project.interrupted()?;

                    let settings = project.configuration(
                        path.parent()
                            .ok_or_else(|| io::Error::other("source has no parent directory"))?,
                    )?;

                    if matches!(
                        settings.configuration.editor.diagnostics.scope,
                        Scope::Workspace
                    ) {
                        selected.push(path);
                    }
                }

                Ok(selected)
            },
            options,
        )
    }
}

/// Discovers Lua and Luau files using the shared filesystem ignore policy.
///
/// # Errors
/// Returns filesystem traversal or interruption failures.
pub fn files(root: &Path, options: &Options) -> io::Result<Vec<PathBuf>> {
    let started = Instant::now();
    let mut paths = BTreeSet::new();

    for entry in ignore::WalkBuilder::new(root).require_git(false).build() {
        options.check(started)?;
        let entry = entry.map_err(io::Error::other)?;

        if entry.file_type().is_some_and(|kind| kind.is_file())
            && matches!(
                entry
                    .path()
                    .extension()
                    .and_then(|extension| extension.to_str()),
                Some("lua" | "luau")
            )
        {
            paths.insert(entry.into_path());
        }
    }

    options.check(started)?;

    Ok(paths.into_iter().collect())
}
