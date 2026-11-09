use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

use instar_bridge::Frontend;
use instar_core::workspace::Purpose;
use tower_lsp_server::jsonrpc::{Error, Result};

use crate::{configuration::Members, connection::Operation, error::failure, project::Project};

pub(super) fn contains(project: &Project, source: &str, target: &str) -> bool {
    project
        .discovery
        .as_ref()
        .and_then(|modules| modules.get(target))
        .is_some_and(|boundary| {
            boundary
                .as_ref()
                .is_none_or(|directory| Path::new(source).starts_with(directory))
        })
}

pub(super) fn members(project: &Project, target: &str, is_imported: bool) -> Result<bool> {
    Ok(match project.configuration.imports.members {
        Members::All => true,
        Members::None => false,
        Members::Imported => is_imported,

        Members::Project => {
            is_imported
                || project
                    .workspace
                    .imports(target)
                    .map_err(failure)?
                    .dependency_root(Path::new(target))
                    .is_none()
        }
    })
}

pub(super) fn pending(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    operation: &Operation,
) -> Result<Vec<(String, bool)>> {
    if !project.configuration.imports.is_enabled {
        return Ok(Vec::new());
    }

    refresh(project, operation)?;

    if project.configuration.imports.members == Members::None {
        return Ok(Vec::new());
    }

    let sources: Vec<_> = project
        .documents
        .iter()
        .filter(|(_, document)| document.version.is_some())
        .map(|(module, _)| module.clone())
        .collect();

    let mut modules: BTreeMap<_, _> = project
        .discovery
        .iter()
        .flat_map(|modules| modules.iter())
        .filter(|(_, boundary)| {
            boundary.as_ref().is_none_or(|directory| {
                sources
                    .iter()
                    .any(|source| Path::new(source).starts_with(directory))
            })
        })
        .map(|(module, _)| (module.clone(), false))
        .collect();

    for source in sources {
        for import in frontend.imports(&source).map_err(failure)? {
            if import.location.module == source {
                modules.insert(import.target, true);
            }
        }
    }

    let mut pending = Vec::new();

    for (module, is_imported) in modules {
        if eligible(project, &module, is_imported)? {
            pending.push((module, is_imported));
        }
    }

    Ok(pending)
}

pub(super) fn collect(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    module: String,
    is_imported: bool,
) -> Result<()> {
    if eligible(project, &module, is_imported)? {
        let exports = frontend.exports(&module).map_err(failure)?;
        project.exports.insert(module, exports);
    }

    Ok(())
}

fn eligible(project: &Project, module: &str, is_imported: bool) -> Result<bool> {
    Ok(project.configuration.imports.is_enabled
        && !project.exports.contains_key(module)
        && members(project, module, is_imported)?
        && project
            .workspace
            .imports(module)
            .map_err(failure)?
            .can_import(Path::new(module))
        && !project
            .workspace
            .is_excluded(module, Purpose::Index)
            .map_err(failure)?)
}

fn refresh(project: &mut Project, operation: &Operation) -> Result<()> {
    if project.discovery.is_some()
        && project.has_capability("/workspace/didChangeWatchedFiles/dynamicRegistration")
    {
        return Ok(());
    }

    let modules = project.index()?;
    let mut directories = BTreeMap::new();
    let mut discovery = BTreeMap::new();

    for module in modules {
        if operation.is_cancelled() {
            return Err(Error::request_cancelled());
        }

        let path = Path::new(&module);
        let mut boundary = None;

        if let Some(root) = project
            .workspace
            .imports(&module)
            .map_err(failure)?
            .dependency_root(path)
        {
            let mut ancestors: Vec<_> = path
                .parent()
                .into_iter()
                .flat_map(Path::ancestors)
                .take_while(|directory| *directory != root)
                .collect();

            ancestors.reverse();

            for (index, directory) in ancestors.iter().enumerate() {
                if directory
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(['.', '_']))
                {
                    boundary = Some(
                        ancestors
                            .get(index + 1)
                            .copied()
                            .unwrap_or(directory)
                            .to_path_buf(),
                    );

                    break;
                }

                let is_module = if let Some(value) = directories.get(*directory) {
                    *value
                } else {
                    let mut found = false;

                    for name in ["init.luau", "init.lua"] {
                        let entry = directory.join(name);

                        found |= project
                            .documents
                            .keys()
                            .any(|module| Path::new(module) == entry);

                        match fs::metadata(&entry) {
                            Ok(metadata) => found |= metadata.is_file(),

                            Err(error)
                                if matches!(
                                    error.kind(),
                                    ErrorKind::NotFound | ErrorKind::NotADirectory
                                ) => {}

                            Err(error) => return Err(failure(error)),
                        }
                    }

                    directories.insert(directory.to_path_buf(), found);

                    found
                };

                if is_module {
                    if path.parent() != Some(directory)
                        || !matches!(
                            path.file_name().and_then(|name| name.to_str()),
                            Some("init.luau" | "init.lua")
                        )
                    {
                        boundary = Some(directory.to_path_buf());
                    }

                    break;
                }
            }
        }

        discovery.insert(module, boundary);
    }

    project.discovery = Some(discovery);

    Ok(())
}
