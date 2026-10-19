use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    rc::Rc,
    slice,
    sync::Arc,
};

use instar_bridge::Frontend;

use instar_core::{
    analysis::{self, Edit, ImportExpression},
    workspace::Workspace,
};

use tower_lsp_server::{
    gen_lsp_types::{DeleteFilesParams, RenameFilesParams, Uri, WorkspaceEdit},
    jsonrpc::{Error, Result},
};

use super::{
    document::{self, Document},
    project::{Project, failure},
    workspace::State,
};

struct Relocation {
    root: PathBuf,
    workspace: Rc<Workspace>,
    documents: BTreeMap<String, Document>,
    retained: bool,
}

pub(super) fn changed(state: &mut State, paths: &[PathBuf]) -> Result<bool> {
    let paths = paths
        .iter()
        .map(|path| identity(path))
        .collect::<Result<Vec<_>>>()?;

    state.observe(&paths)?;
    let mut changed = false;

    for project in state.projects.values_mut() {
        if !affected(project, &paths)? {
            continue;
        }

        let mut relevant = Vec::new();

        for path in &paths {
            if affected(project, slice::from_ref(path))? && !unchanged(project, path)? {
                relevant.push(path.clone());
            }
        }

        if relevant.is_empty() {
            continue;
        }

        let dependencies = project.reload_dependencies();
        invalidate(project, &relevant, &dependencies)?;
        changed = true;
    }

    Ok(changed)
}

pub(super) fn invalidate(
    project: &mut Project,
    paths: &[PathBuf],
    dependencies: &BTreeSet<PathBuf>,
) -> Result<()> {
    let reload = paths.iter().any(|path| {
        path.file_name().is_some_and(|name| name == "instar.toml")
            && path.starts_with(project.workspace.directory())
    }) || dependencies
        .iter()
        .map(|path| identity(path))
        .collect::<Result<Vec<_>>>()?
        .iter()
        .any(|dependency| paths.iter().any(|path| intersects(dependency, path)));

    if reload {
        project.reload()?;

        return Ok(());
    }

    project.changed()?;

    let modules: BTreeSet<_> = project
        .dependencies()
        .into_iter()
        .chain(project.documents.keys().map(PathBuf::from))
        .chain(project.borrowed.keys().map(PathBuf::from))
        .filter(|module| paths.iter().any(|path| module.starts_with(path)))
        .collect();

    for module in modules {
        let module = project.workspace.normalize(&module).map_err(failure)?;

        if project
            .documents
            .get(&module)
            .or_else(|| project.borrowed.get(&module))
            .is_none_or(|document| document.version.is_none())
        {
            project.workspace.remove_source(&module);
        }

        project.invalidate(&module);
    }

    project.tokens.clear();

    Ok(())
}

pub(super) fn renamed(state: &mut State, parameters: RenameFilesParams) -> Result<Vec<Uri>> {
    let mappings = mappings(parameters, false)?;

    if mappings.is_empty() {
        return Ok(Vec::new());
    }

    let paths: Vec<_> = mappings
        .iter()
        .flat_map(|(old, new)| [old.clone(), new.clone()])
        .collect();

    let roots: BTreeMap<_, _> = state
        .projects
        .keys()
        .map(|root| (root.clone(), relocated(root, &mappings)))
        .collect();

    let mut unique = BTreeSet::new();

    for root in roots.values() {
        if !unique.insert(root.clone()) {
            return Err(Error::invalid_params("renamed workspace roots collide"));
        }
    }

    let mut documents: BTreeMap<PathBuf, BTreeMap<String, Document>> = BTreeMap::new();
    let mut affected_roots = BTreeSet::new();

    for (root, project) in &state.projects {
        if affected(project, &paths)? || roots[root] != *root {
            affected_roots.insert(root.clone());
        }

        for (module, document) in &project.documents {
            if document.version.is_none() {
                continue;
            }

            let original = Path::new(module);
            let module = relocated(original, &mappings);

            let destination = roots
                .iter()
                .filter(|(original, root)| {
                    state.projects[*original].indexed && module.starts_with(root)
                })
                .max_by_key(|(_, root)| root.components().count())
                .map_or(root, |(original, _)| original);

            if destination != root || module != original {
                affected_roots.insert(root.clone());
                affected_roots.insert(destination.clone());
            }

            let mut document = document.clone();

            if module != original {
                document.uri = document::uri(&module)?;
            }

            let module = project.workspace.normalize(&module).map_err(failure)?;

            if documents
                .entry(destination.clone())
                .or_default()
                .insert(module, document)
                .is_some()
            {
                return Err(Error::invalid_params("renamed open documents collide"));
            }
        }
    }

    let mut relocations = BTreeMap::new();
    let mut cleared = BTreeSet::new();

    for root in affected_roots {
        let project = state
            .projects
            .get_mut(&root)
            .expect("internal error: renamed project missing");

        let destination = roots[&root].clone();
        let retained = destination == root;

        let workspace = if retained {
            project.reload()?;

            Rc::clone(&project.workspace)
        } else {
            let workspace = Workspace::new(&destination, &[], &[]).map_err(failure)?;
            project.changed()?;
            project.invalidation = project.generation;

            Rc::new(workspace)
        };

        let documents = documents.remove(&root).unwrap_or_default();

        if !retained {
            for (module, document) in &documents {
                let identity = workspace
                    .set_source(Path::new(module), Arc::clone(&document.text))
                    .map_err(failure)?;

                if identity != *module {
                    return Err(Error::content_modified());
                }
            }
        }

        for module in &project.published {
            cleared.insert(document::uri(Path::new(module))?);
        }

        relocations.insert(
            root,
            Relocation {
                root: destination,
                workspace,
                documents,
                retained,
            },
        );
    }

    let mut overlays = Vec::new();

    for (root, relocation) in &relocations {
        if !relocation.retained {
            continue;
        }

        let project = &state.projects[root];

        for (module, document) in &relocation.documents {
            if project.documents.get(module).is_some_and(|previous| {
                previous.version.is_some() && previous.text == document.text
            }) {
                continue;
            }

            let result = relocation
                .workspace
                .set_source(Path::new(module), Arc::clone(&document.text))
                .map_err(failure);

            let result = match result {
                Ok(identity) => {
                    let previous = project
                        .documents
                        .get(&identity)
                        .or_else(|| project.borrowed.get(&identity))
                        .filter(|document| document.version.is_some())
                        .map(|document| Arc::clone(&document.text));

                    let matches = identity == *module;
                    overlays.push((root.clone(), identity, previous));

                    if matches {
                        Ok(())
                    } else {
                        Err(Error::content_modified())
                    }
                }

                Err(error) => Err(error),
            };

            if let Err(error) = result {
                restore(state, overlays)?;

                return Err(error);
            }
        }
    }

    let mut replacements = Vec::new();

    for (root, relocation) in relocations {
        let mut project = state
            .projects
            .remove(&root)
            .expect("internal error: renamed project missing");

        project.workspace = relocation.workspace;

        if !relocation.retained {
            project.borrowed.clear();
        }

        if relocation.retained {
            for module in project
                .documents
                .keys()
                .filter(|module| !relocation.documents.contains_key(*module))
            {
                project.workspace.remove_source(module);
            }
        }

        project.documents.clear();

        for (module, document) in relocation.documents {
            project.adopt(module, document);
        }

        project.tokens.clear();
        project.published.clear();
        replacements.push((relocation.root, project));
    }

    state.projects.extend(replacements);
    state.reconcile()?;

    Ok(cleared.into_iter().collect())
}

pub(super) fn restore(
    state: &State,
    overlays: Vec<(PathBuf, String, Option<Arc<str>>)>,
) -> Result<()> {
    let mut restore_error = None;

    for (root, module, previous) in overlays.into_iter().rev() {
        let workspace = &state.projects[&root].workspace;

        let result = if let Some(previous) = previous {
            workspace
                .set_source(Path::new(&module), previous)
                .map(|_| ())
                .map_err(failure)
        } else {
            workspace.remove_source(&module);

            Ok(())
        };

        if let Err(error) = result
            && restore_error.is_none()
        {
            restore_error = Some(error);
        }
    }

    restore_error.map_or(Ok(()), Err)
}

pub(super) fn deleted(state: &mut State, parameters: DeleteFilesParams) -> Result<Vec<Uri>> {
    let paths = parameters
        .files
        .into_iter()
        .map(|file| path(&file.uri).and_then(|path| identity(&path)))
        .collect::<Result<Vec<_>>>()?;

    let mut cleared = BTreeSet::new();

    for project in state.projects.values() {
        for module in &project.published {
            if paths.iter().any(|path| Path::new(module).starts_with(path)) {
                cleared.insert(document::uri(Path::new(module))?);
            }
        }
    }

    changed(state, &paths)?;

    for project in state.projects.values_mut() {
        project
            .published
            .retain(|module| !paths.iter().any(|path| Path::new(module).starts_with(path)));
    }

    Ok(cleared.into_iter().collect())
}

pub(super) fn will_rename(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: RenameFilesParams,
) -> Result<Option<WorkspaceEdit>> {
    let mappings = mappings(parameters, true)?;

    if mappings.is_empty() {
        return Ok(None);
    }

    let mut changes = BTreeMap::new();

    for module in project.index()? {
        let document = project.snapshot(&module)?;
        let future_module = relocated(Path::new(&module), &mappings);
        let moved = future_module != Path::new(&module);
        let mut imports: BTreeMap<(usize, usize), BTreeSet<(String, String)>> = BTreeMap::new();

        for import in frontend.imports(&module).map_err(failure)? {
            if import.location.module != module || !Path::new(&import.target).is_absolute() {
                return Err(Error::content_modified());
            }

            let range = document.native_range(import.location.range, project.encoding)?;
            let range = document.bytes(range, project.encoding)?;

            imports
                .entry((range.start, range.end))
                .or_default()
                .insert((import.target, import.identity));
        }

        if project.workspace.snapshot(&module).as_deref() != Some(document.text.as_ref()) {
            return Err(Error::content_modified());
        }

        if moved
            && document.analysis.imports.iter().any(|import| {
                !imports
                    .keys()
                    .any(|(start, end)| *start <= import.range.start && import.range.end <= *end)
            })
        {
            return Err(failure(
                "cannot safely update an unresolved import in a moved source",
            ));
        }

        let mut edits = Vec::new();

        for ((start, end), targets) in imports {
            if !moved
                && targets
                    .iter()
                    .all(|(target, _)| relocated(Path::new(target), &mappings) == Path::new(target))
            {
                continue;
            }

            if targets.len() != 1 {
                return Err(failure(
                    "cannot safely update an import with multiple runtime targets",
                ));
            }

            let (target, identity) = targets
                .into_iter()
                .next()
                .expect("internal error: import target missing");

            let future_target = relocated(Path::new(&target), &mappings);

            let mut styles = document
                .analysis
                .imports
                .iter()
                .filter(|import| start <= import.range.start && import.range.end <= end);

            let style = styles
                .next()
                .ok_or_else(|| failure("cannot safely update an unsupported import expression"))?;

            if styles.next().is_some() {
                return Err(failure(
                    "cannot safely update overlapping import expressions",
                ));
            }

            let instance = matches!(style.expression, ImportExpression::Instance(_));

            if matches!(&style.expression, ImportExpression::Path(path)
                if path.split('/').next().is_some_and(|alias| alias.eq_ignore_ascii_case("@game")))
                && identity != target
            {
                return Err(failure(
                    "cannot safely update an instance-backed string import before its sourcemap is updated",
                ));
            }

            let future_module = project
                .workspace
                .normalize(&future_module)
                .map_err(failure)?;

            let future_target = project
                .workspace
                .normalize(&future_target)
                .map_err(failure)?;

            let replacement = project
                .workspace
                .import(&future_module, &future_target, instance)
                .map_err(failure)?
                .ok_or_else(|| {
                    failure(if instance {
                        "cannot safely update an instance import before its sourcemap is updated"
                    } else {
                        "cannot generate an unambiguous import for the renamed source"
                    })
                })?;

            if instance && !unshadowed(&document, &replacement) {
                return Err(failure(
                    "renamed instance import would use a shadowed or written global",
                ));
            }

            let expected = document
                .text
                .get(start..end)
                .ok_or_else(Error::content_modified)?;

            if expected != replacement {
                edits.push(Edit {
                    range: start..end,
                    replacement,
                });
            }
        }

        if edits.is_empty() {
            continue;
        }

        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));

        if edits
            .windows(2)
            .any(|pair| pair[0].range.end > pair[1].range.start)
        {
            return Err(Error::invalid_params("renamed import edits overlap"));
        }

        analysis::validate(&document.text).map_err(failure)?;
        let mut proposed = document.text.to_string();

        for edit in edits.iter().rev() {
            proposed.replace_range(edit.range.clone(), &edit.replacement);
        }

        analysis::validate(&proposed).map_err(failure)?;
        changes.insert(module, edits);
    }

    if changes.is_empty() {
        Ok(None)
    } else {
        project.edits(changes).map(Some)
    }
}

fn affected(project: &Project, paths: &[PathBuf]) -> Result<bool> {
    if paths.iter().any(|path| {
        intersects(project.workspace.directory(), path)
            && (path
                .extension()
                .is_some_and(|extension| extension == "luau" || extension == "lua")
                || path.file_name().is_some_and(|name| name == "instar.toml")
                || path.is_dir()
                || project.workspace.directory().starts_with(path))
    }) {
        return Ok(true);
    }

    for dependency in project.dependencies() {
        let dependency = identity(&dependency)?;

        if paths.iter().any(|path| intersects(&dependency, path)) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn unchanged(project: &Project, path: &Path) -> Result<bool> {
    for dependency in project.reload_dependencies() {
        if intersects(&identity(&dependency)?, path) {
            return Ok(false);
        }
    }

    let module = project.workspace.normalize(path).map_err(failure)?;

    let document = project
        .documents
        .get(&module)
        .or_else(|| project.borrowed.get(&module));

    if document.is_some_and(|document| document.version.is_some()) {
        return Ok(true);
    }

    let previous = document
        .map(|document| Arc::clone(&document.text))
        .or_else(|| project.workspace.snapshot(&module))
        .or_else(|| project.observed.get(&module).map(Arc::clone));

    let Some(previous) = previous else {
        return Ok(false);
    };

    match fs::read_to_string(path) {
        Ok(current) => Ok(current == previous.as_ref()),

        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(false)
        }

        Err(error) => Err(failure(error)),
    }
}

fn mappings(parameters: RenameFilesParams, before: bool) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut mappings = Vec::new();

    for file in parameters.files {
        let old = path(&file.old_uri)?;

        let old = if before {
            identity(&old)?
        } else {
            lexical(&old)?
        };

        let new = destination(&path(&file.new_uri)?)?;

        if old == new {
            continue;
        }

        if new.starts_with(&old) {
            return Err(Error::invalid_params(
                "a source cannot be moved inside itself",
            ));
        }

        mappings.push((old, new));
    }

    for (index, (old, new)) in mappings.iter().enumerate() {
        if mappings[..index]
            .iter()
            .any(|(previous_old, previous_new)| {
                intersects(old, previous_old) || intersects(new, previous_new)
            })
        {
            return Err(Error::invalid_params(
                "file rename mappings overlap or collide",
            ));
        }

        if before
            && new.try_exists().map_err(failure)?
            && !mappings.iter().any(|(source, _)| new.starts_with(source))
            && identity(new)? != identity(old)?
        {
            return Err(Error::invalid_params(
                "file rename destination already exists",
            ));
        }
    }

    Ok(mappings)
}

fn relocated(path: &Path, mappings: &[(PathBuf, PathBuf)]) -> PathBuf {
    mappings
        .iter()
        .find_map(|(old, new)| {
            path.strip_prefix(old).ok().map(|remainder| {
                if remainder.as_os_str().is_empty() {
                    new.clone()
                } else {
                    new.join(remainder)
                }
            })
        })
        .unwrap_or_else(|| path.to_owned())
}

fn intersects(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn path(uri: &Uri) -> Result<PathBuf> {
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(Error::invalid_params(
            "file operation URIs cannot contain queries or fragments",
        ));
    }

    document::path(uri)
}

fn destination(path: &Path) -> Result<PathBuf> {
    let path = lexical(path)?;

    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(Error::invalid_params("file rename destination has no name"));
    };

    Ok(identity(parent)?.join(name))
}

pub(super) fn identity(path: &Path) -> Result<PathBuf> {
    let normalized = lexical(path)?;
    let mut ancestor = normalized.as_path();
    let mut remainder = Vec::new();

    while !ancestor.try_exists().map_err(failure)? {
        let (Some(parent), Some(name)) = (ancestor.parent(), ancestor.file_name()) else {
            return Err(Error::invalid_params(
                "file operation path has no existing ancestor",
            ));
        };

        remainder.push(name);
        ancestor = parent;
    }

    let mut canonical = ancestor.canonicalize().map_err(failure)?;

    for component in remainder.into_iter().rev() {
        canonical.push(component);
    }

    lexical(&canonical)
}

fn lexical(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() || path.to_str().is_none_or(|path| path.contains('\0')) {
        return Err(Error::invalid_params(
            "file operation paths must be absolute UTF-8 paths",
        ));
    }

    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::CurDir => {}

            Component::ParentDir => {
                normalized.pop();
            }

            component => normalized.push(component.as_os_str()),
        }
    }

    let text = normalized
        .to_str()
        .ok_or_else(|| Error::invalid_params("file operation path is not UTF-8"))?;

    #[cfg(windows)]
    let text = if let Some(network) = text.strip_prefix(r"\\?\UNC\") {
        format!("//{}", network.replace('\\', "/"))
    } else {
        text.strip_prefix(r"\\?\")
            .unwrap_or(text)
            .replace('\\', "/")
    };

    Ok(PathBuf::from(text))
}

fn unshadowed(document: &Document, expression: &str) -> bool {
    let root = expression.split(['.', ':', '[']).next().unwrap_or_default();

    matches!(root, "script" | "game" | "workspace")
        && !document
            .analysis
            .bindings
            .iter()
            .any(|binding| binding.name == root)
        && !document.analysis.tokens.iter().any(|token| {
            token.access.write() && document.text.get(token.range.clone()) == Some(root)
        })
}
