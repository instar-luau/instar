use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

use super::{Error, Workspace, model::Scope, paths};
use crate::settings::{defaults, layer};

pub(super) const CONFIGURATION: &str = "instar.toml";

/// Return configuration paths in precedence order, including missing discovery candidates.
#[must_use]
pub fn configuration_files(root: &Path, files: &[PathBuf]) -> Vec<PathBuf> {
    if files.is_empty() {
        let mut paths: Vec<_> = root
            .ancestors()
            .map(|directory| directory.join(CONFIGURATION))
            .collect();

        paths.reverse();

        paths
    } else {
        files.iter().map(|file| root.join(file)).collect()
    }
}

pub(super) fn root(workspace: &Workspace) -> Result<Scope, Error> {
    let mut layers = defaults();

    for file in configuration_files(&workspace.root, &workspace.files) {
        if !workspace.files.is_empty() || paths::metadata(&file)?.is_some() {
            layers = layer(layers, &file);
        }
    }

    Scope::new(&workspace.root, layers, &workspace.overrides)
}

pub(super) fn directory(workspace: &Workspace, directory: &Path) -> Result<Rc<Scope>, Error> {
    if !workspace.files.is_empty() {
        return Ok(workspace.root());
    }

    let mut pending = Vec::new();
    let mut scope = None;

    for ancestor in directory.ancestors() {
        if let Some(existing) = workspace.scopes.borrow().get(ancestor) {
            scope = Some(Rc::clone(existing));
            break;
        }

        pending.push(ancestor);
    }

    workspace.configuration_paths.borrow_mut().extend(
        pending
            .iter()
            .map(|directory| directory.join(CONFIGURATION)),
    );

    let mut scope = match scope {
        Some(scope) => scope,

        None => Rc::new(Scope::new(
            &workspace.root,
            defaults(),
            &workspace.overrides,
        )?),
    };

    for directory in pending.into_iter().rev() {
        let file = directory.join(CONFIGURATION);

        if paths::metadata(&file)?.is_some() {
            scope = Rc::new(Scope::new(
                &workspace.root,
                layer(scope.layers.clone(), &file),
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
