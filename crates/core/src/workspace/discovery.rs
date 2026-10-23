use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use figment::Figment;
use instar_bridge::Language;

use super::{Error, Workspace, model::Scope, paths};
use crate::settings::layer;

const CONFIGURATIONS: [&str; 3] = ["instar.toml", ".luaurc", ".config.luau"];

/// Return configuration discovery candidates, including missing files.
#[must_use]
pub fn configuration_files(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    let directories: Vec<_> = root.ancestors().collect();

    let mut paths: Vec<_> = directories
        .iter()
        .rev()
        .flat_map(|directory| CONFIGURATIONS[1..].iter().map(|name| directory.join(name)))
        .collect();

    if files.is_empty() {
        paths.extend(
            directories
                .iter()
                .rev()
                .map(|directory| directory.join(CONFIGURATIONS[0])),
        );
    } else {
        paths.extend(files.iter().map(|file| root.join(file)));
    }

    paths
}

/// Identify a project or native language configuration file.
#[must_use]
pub fn is_configuration(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        CONFIGURATIONS.iter().any(|candidate| name == *candidate)
            || name
                .to_str()
                .is_some_and(|name| name.ends_with(".project.json"))
    })
}

pub(super) fn root(workspace: &Workspace) -> Result<Scope, Error> {
    let mut layers = Figment::new();
    let mut language = None;
    let directories: Vec<_> = workspace.root.ancestors().collect();

    for directory in directories.into_iter().rev() {
        if let Some(loaded) = load_language(directory, language.as_ref())? {
            language = Some(loaded);
        }

        let file = directory.join(CONFIGURATIONS[0]);

        if workspace.files.is_empty() && paths::metadata(&file)?.is_some() {
            layers = layer(layers, &file);
        }
    }

    for file in &workspace.files {
        layers = layer(layers, &workspace.root.join(file));
    }

    Scope::new(&workspace.root, layers, language, &workspace.overrides)
}

pub(super) fn directory(workspace: &Workspace, directory: &Path) -> Result<Rc<Scope>, Error> {
    let mut pending = Vec::new();
    let mut scope = None;

    for ancestor in directory.ancestors() {
        if let Some(existing) = workspace.scopes.borrow().get(ancestor) {
            scope = Some(Rc::clone(existing));
            break;
        }

        pending.push(ancestor);
    }

    if !pending.is_empty() {
        workspace.observations.borrow_mut().take();
    }

    workspace
        .configuration_paths
        .borrow_mut()
        .extend(pending.iter().flat_map(|directory| {
            CONFIGURATIONS
                .iter()
                .filter(|name| workspace.files.is_empty() || **name != CONFIGURATIONS[0])
                .map(|name| directory.join(name))
        }));

    let mut scope = match scope {
        Some(scope) => scope,

        None => Rc::new(Scope::new(
            &workspace.root,
            if workspace.files.is_empty() {
                Figment::new()
            } else {
                workspace.root().layers.clone()
            },
            None,
            &workspace.overrides,
        )?),
    };

    for directory in pending.into_iter().rev() {
        let file = directory.join(CONFIGURATIONS[0]);
        let language = load_language(directory, scope.language.as_ref())?;
        let has_project = workspace.files.is_empty() && paths::metadata(&file)?.is_some();

        if has_project || language.is_some() {
            let layers = if has_project {
                layer(scope.layers.clone(), &file)
            } else {
                scope.layers.clone()
            };

            scope = Rc::new(Scope::new(
                &workspace.root,
                layers,
                language.or_else(|| scope.language.clone()),
                &workspace.overrides,
            )?);
        }

        workspace
            .scopes
            .borrow_mut()
            .insert(directory.to_owned(), Rc::clone(&scope));
    }

    Ok(scope)
}

fn load_language(directory: &Path, previous: Option<&Language>) -> Result<Option<Language>, Error> {
    let json = directory.join(CONFIGURATIONS[1]);
    let executable = directory.join(CONFIGURATIONS[2]);
    let has_json = paths::metadata(&json)?.is_some();
    let has_executable = paths::metadata(&executable)?.is_some();

    if has_json && has_executable {
        return Err(Error::Path {
            path: directory.to_owned(),
            reason: "Both `.luaurc` and `.config.luau` exist.",
        });
    }

    if !has_json && !has_executable {
        return Ok(None);
    }

    let path = if has_executable { executable } else { json };

    let source = paths::read(&path)?;
    let name = paths::text(&path)?;

    let configuration = Language::parse(previous, &name, &source, has_executable)
        .map_err(|source| Error::Language { path, source })?;

    Ok(Some(configuration))
}
