use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
    rc::Rc,
};

use globset::{Glob, GlobBuilder, GlobMatcher, escape};
use ignore::WalkBuilder;

use super::{Error, Workspace, paths, process};
use crate::settings::{Configuration, Patterns};

/// The command whose file-selection settings apply during discovery.
#[derive(Copy, Clone)]
pub enum Purpose {
    /// Apply checking inclusion and exclusion patterns.
    Check,

    /// Apply formatting inclusion and exclusion patterns.
    Format,

    /// Apply lint inclusion and exclusion patterns.
    Lint,

    /// Apply indexing inclusion and exclusion patterns.
    Index,
}

/// The access required by the operation selecting sources.
#[derive(Copy, Clone)]
pub enum Access {
    /// Select sources for reading.
    Read,

    /// Reject explicit symbolic links before selecting write destinations.
    Write,
}

pub(super) struct Selection<'configuration> {
    include: Option<&'configuration Patterns>,
    exclude: Option<&'configuration Patterns>,
}

impl<'configuration> Selection<'configuration> {
    pub(super) fn new(configuration: &'configuration Configuration, purpose: Purpose) -> Self {
        let (include, exclude) = match purpose {
            Purpose::Check => (&configuration.check.include, &configuration.check.exclude),
            Purpose::Format => (&configuration.format.include, &configuration.format.exclude),
            Purpose::Lint => (&configuration.lint.include, &configuration.lint.exclude),
            Purpose::Index => (&configuration.index.include, &configuration.index.exclude),
        };

        Self {
            include: include.as_ref().or(configuration.include.as_ref()),
            exclude: exclude.as_ref().or(configuration.exclude.as_ref()),
        }
    }

    fn matches(&self, path: &Path) -> bool {
        self.include
            .is_none_or(|include| include.matches(path, true))
            && !self.excluded(path)
    }

    pub(super) fn excluded(&self, path: &Path) -> bool {
        self.exclude
            .is_some_and(|exclude| exclude.matches(path, false))
    }
}

pub(super) fn select(
    workspace: &Workspace,
    inputs: &[PathBuf],
    purpose: Purpose,
    access: Access,
) -> Result<BTreeMap<String, PathBuf>, Error> {
    let mut selected = BTreeMap::new();
    let cancellation = Rc::clone(&workspace.cancelled.borrow());
    let cancelled = cancellation.as_ref();
    process::interrupted(cancelled)?;

    if inputs.is_empty() {
        walk(workspace, &workspace.root, None, purpose, &mut selected)?;
    }

    for input in inputs {
        process::interrupted(cancelled)?;
        let path = workspace.root.join(input);
        let input = paths::text(input)?;

        let metadata = if cfg!(windows) && input.contains(['*', '?']) {
            None
        } else {
            paths::metadata(&path)?
        };

        if let Some(metadata) = metadata {
            if metadata.is_dir() {
                walk(
                    workspace,
                    &paths::canonical(&path)?,
                    None,
                    purpose,
                    &mut selected,
                )?;
            } else {
                if matches!(access, Access::Write)
                    && fs::symlink_metadata(&path)
                        .map_err(|source| Error::Filesystem {
                            path: path.clone(),
                            source,
                        })?
                        .file_type()
                        .is_symlink()
                {
                    return Err(Error::Path {
                        path,
                        reason: "refusing to write a symbolic link",
                    });
                }

                let module = workspace.module(&path)?;
                let input = PathBuf::from(&module);

                if !matches!(purpose, Purpose::Index)
                    || Selection::new(&workspace.scope(&input)?.configuration, purpose)
                        .matches(&input)
                {
                    selected.entry(module).or_insert(input);
                }
            }
        } else {
            if !input.contains(['*', '?', '[', '{']) {
                if matches!((purpose, access), (Purpose::Index, Access::Read)) {
                    let module = paths::text(&paths::identity(&path)?)?;

                    if workspace.sources.borrow().contains_key(&module) {
                        let input = PathBuf::from(&module);

                        if Selection::new(&workspace.scope(&input)?.configuration, purpose)
                            .matches(&input)
                        {
                            selected.entry(module).or_insert(input);
                        }

                        continue;
                    }
                }

                return Err(Error::Path {
                    path,
                    reason: "input does not exist",
                });
            }

            let matcher = pattern(&workspace.root, &input)?.compile_matcher();

            let prefix: PathBuf = Path::new(&input)
                .components()
                .take_while(|component| {
                    !component
                        .as_os_str()
                        .to_string_lossy()
                        .contains(['*', '?', '[', '{'])
                })
                .collect();

            let directory = workspace.root.join(prefix);

            if paths::metadata(&directory)?.is_some_and(|metadata| metadata.is_dir()) {
                walk(
                    workspace,
                    &paths::canonical(&directory)?,
                    Some(&matcher),
                    purpose,
                    &mut selected,
                )?;
            }
        }
    }

    Ok(selected)
}

fn walk(
    workspace: &Workspace,
    root: &Path,
    pattern: Option<&GlobMatcher>,
    purpose: Purpose,
    selected: &mut BTreeMap<String, PathBuf>,
) -> Result<(), Error> {
    let cancellation = Rc::clone(&workspace.cancelled.borrow());
    let cancelled = cancellation.as_ref();

    for entry in WalkBuilder::new(root).standard_filters(false).build() {
        process::interrupted(cancelled)?;
        let entry = entry.map_err(Error::Traversal)?;
        let path = entry.path();

        if entry.file_type().is_some_and(|kind| kind.is_file())
            && paths::source(path)
            && pattern.is_none_or(|pattern| pattern.is_match(path))
            && Selection::new(&workspace.scope(path)?.configuration, purpose).matches(path)
        {
            selected
                .entry(workspace.module(path)?)
                .or_insert_with(|| root.to_owned());
        }
    }

    if matches!(purpose, Purpose::Index) {
        for module in workspace.sources.borrow().keys() {
            process::interrupted(cancelled)?;
            let path = Path::new(module);

            if path.starts_with(root)
                && pattern.is_none_or(|pattern| pattern.is_match(path))
                && Selection::new(&workspace.scope(path)?.configuration, purpose).matches(path)
            {
                selected
                    .entry(module.clone())
                    .or_insert_with(|| root.to_owned());
            }
        }
    }

    Ok(())
}

fn pattern(base: &Path, input: &str) -> Result<Glob, Error> {
    let path = Path::new(input);

    let mut prefix = if path.is_absolute() {
        PathBuf::new()
    } else {
        base.to_owned()
    };

    let mut components = path.components().peekable();

    while let Some(component) = components.next_if(|component| {
        !component
            .as_os_str()
            .to_string_lossy()
            .contains(['*', '?', '[', '{'])
    }) {
        match component {
            Component::ParentDir => {
                prefix.pop();
            }

            Component::CurDir => {}
            _ => prefix.push(component.as_os_str()),
        }
    }

    let remainder: PathBuf = components.collect();

    let pattern = if remainder.as_os_str().is_empty() {
        escape(&paths::text(&prefix)?)
    } else {
        paths::text(&Path::new(&escape(&paths::text(&prefix)?)).join(remainder))?
    };

    GlobBuilder::new(&pattern)
        .literal_separator(true)
        .build()
        .map_err(Error::Pattern)
}
