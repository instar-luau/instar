use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use globset::{Glob, GlobBuilder, GlobMatcher, GlobSet, GlobSetBuilder, escape};
use ignore::WalkBuilder;

use super::{Error, Workspace, paths};

/// The command whose file-selection settings apply during discovery.
#[derive(Clone, Copy)]
pub enum Purpose {
    /// Apply checking inclusion and exclusion patterns.
    Check,

    /// Apply formatting inclusion and exclusion patterns.
    Format,

    /// Apply lint inclusion and exclusion patterns.
    Lint,
}

/// The access required by the operation selecting sources.
#[derive(Clone, Copy)]
pub enum Access {
    /// Select sources for reading.
    Read,

    /// Reject explicit symbolic links before selecting write destinations.
    Write,
}

pub(super) struct Selection {
    include: Option<GlobSet>,
    exclude: GlobSet,
}

impl Selection {
    pub(super) fn new(
        base: &Path,
        include: Option<&[String]>,
        exclude: Option<&[String]>,
    ) -> Result<Self, Error> {
        Ok(Self {
            include: include
                .map(|patterns| compile(base, patterns))
                .transpose()?,
            exclude: compile(base, exclude.unwrap_or_default())?,
        })
    }

    fn matches(&self, path: &Path) -> bool {
        self.include
            .as_ref()
            .is_none_or(|include| include.is_match(path))
            && !self.exclude.is_match(path)
    }
}

pub(super) fn select(
    workspace: &Workspace,
    inputs: &[PathBuf],
    purpose: Purpose,
    access: Access,
) -> Result<BTreeMap<String, PathBuf>, Error> {
    let mut selected = BTreeMap::new();

    if inputs.is_empty() {
        walk(workspace, &workspace.root, None, purpose, &mut selected)?;
    }

    for input in inputs {
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
                selected.entry(module).or_insert(input);
            }
        } else {
            if !input.contains(['*', '?', '[', '{']) {
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
    for entry in WalkBuilder::new(root).standard_filters(false).build() {
        let entry = entry.map_err(Error::Traversal)?;
        let path = entry.path();

        if entry.file_type().is_some_and(|kind| kind.is_file())
            && paths::source(path)
            && pattern.is_none_or(|pattern| pattern.is_match(path))
            && match purpose {
                Purpose::Check => workspace.scope(path)?.checking.matches(path),
                Purpose::Format => workspace.scope(path)?.formatting.matches(path),
                Purpose::Lint => workspace.scope(path)?.linting.matches(path),
            }
        {
            selected
                .entry(workspace.module(path)?)
                .or_insert_with(|| root.to_owned());
        }
    }

    Ok(())
}

fn compile(base: &Path, patterns: &[String]) -> Result<GlobSet, Error> {
    let mut builder = GlobSetBuilder::new();

    for input in patterns {
        builder.add(pattern(base, input)?);
    }

    builder.build().map_err(Error::Pattern)
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
