use std::{path::Path, rc::Rc};

use super::{Error, Workspace, model::Scope, paths};
use crate::settings::{defaults, layer};

pub(super) fn root(workspace: &Workspace) -> Result<Scope, Error> {
    let mut layers = defaults();

    if workspace.files.is_empty() {
        let mut ancestors: Vec<_> = workspace.root.ancestors().collect();
        ancestors.reverse();

        for directory in ancestors {
            let file = directory.join("instar.toml");

            if paths::metadata(&file)?.is_some() {
                layers = layer(layers, &file);
            }
        }
    } else {
        for file in &workspace.files {
            layers = layer(layers, &workspace.root.join(file));
        }
    }

    Scope::new(&workspace.root, layers, &workspace.overrides)
}

pub(super) fn directory(workspace: &Workspace, directory: &Path) -> Result<Rc<Scope>, Error> {
    if !directory.starts_with(&workspace.root) || !workspace.files.is_empty() {
        return Ok(workspace.root());
    }

    let mut pending = Vec::new();
    let mut scope = workspace.root();

    for ancestor in directory.ancestors() {
        if let Some(existing) = workspace.scopes.borrow().get(ancestor) {
            scope = Rc::clone(existing);
            break;
        }

        pending.push(ancestor);
    }

    for directory in pending.into_iter().rev() {
        let file = directory.join("instar.toml");

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
