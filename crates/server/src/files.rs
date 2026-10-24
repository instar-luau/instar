use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::ErrorKind,
    mem,
    path::{Path, PathBuf},
    rc::Rc,
    slice,
    sync::Arc,
};

use instar_bridge::Frontend;

use instar_core::{
    analysis::{self, Edit, ImportExpression},
    workspace::{Generators, Workspace, is_configuration},
};

use tokio_util::sync::CancellationToken;

use tower_lsp_server::{
    gen_lsp_types::{DeleteFilesParams, RenameFilesParams, Uri, WorkspaceEdit},
    jsonrpc::{Error, Result},
};

use super::{
    document::{self, Document},
    error::{failure, reason},
    paths::{destination, identity, is_intersecting, lexical},
    project::Project,
    workspace::State,
};

struct Relocation {
    root: PathBuf,
    workspace: Rc<Workspace>,
    generators: Generators,
    documents: BTreeMap<String, Document>,
    is_retained: bool,
}

pub(super) fn changed(state: &mut State, paths: &[PathBuf]) -> Result<bool> {
    let paths = paths
        .iter()
        .map(|path| identity(path))
        .collect::<Result<Vec<_>>>()?;

    state.observe(&paths)?;
    let mut has_changed = false;

    for project in state.projects.values_mut() {
        if !is_affected(project, &paths)? {
            continue;
        }

        let mut relevant = Vec::new();

        for path in &paths {
            if is_affected(project, slice::from_ref(path))? && !is_unchanged(project, path)? {
                relevant.push(path.clone());
            }
        }

        if relevant.is_empty() {
            continue;
        }

        let dependencies = project.reload_dependencies();
        invalidate(project, &relevant, &dependencies)?;
        has_changed = true;
    }

    Ok(has_changed)
}

pub(super) fn invalidate(
    project: &mut Project,
    paths: &[PathBuf],
    dependencies: &BTreeSet<PathBuf>,
) -> Result<()> {
    let should_reload = paths
        .iter()
        .any(|path| is_configuration(path) && path.starts_with(project.workspace.directory()))
        || dependencies
            .iter()
            .map(|path| identity(path))
            .collect::<Result<Vec<_>>>()?
            .iter()
            .any(|dependency| paths.iter().any(|path| is_intersecting(dependency, path)));

    if should_reload {
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
            return Err(Error::invalid_params("Renamed workspace roots collide."));
        }
    }

    let mut documents: BTreeMap<PathBuf, BTreeMap<String, Document>> = BTreeMap::new();
    let mut affected_roots = BTreeSet::new();

    for (root, project) in &state.projects {
        if is_affected(project, &paths)? || roots[root] != *root {
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
                    state.projects[*original].should_index && module.starts_with(root)
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
                return Err(Error::invalid_params("Renamed open documents collide."));
            }
        }
    }

    let mut relocations = BTreeMap::new();
    let mut cleared = BTreeSet::new();

    for root in affected_roots {
        let project = state
            .projects
            .get_mut(&root)
            .expect("Internal error: renamed project is missing.");

        let destination = roots[&root].clone();
        let is_retained = destination == root;

        let (workspace, generators) = if is_retained {
            project.reload()?;

            (Rc::clone(&project.workspace), project.generators.clone())
        } else {
            let generators = Generators::new(&CancellationToken::new(), |executable, message| {
                tracing::info!(executable, message, "Sourcemap generator");
            });

            let workspace = Workspace::new(&destination, &[], &[])
                .map_err(failure)?
                .with_generators(generators.clone());

            project.changed()?;
            project.invalidation = project.generation;

            (Rc::new(workspace), generators)
        };

        let documents = documents.remove(&root).unwrap_or_default();

        if !is_retained {
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
                generators,
                documents,
                is_retained,
            },
        );
    }

    let mut overlays = Vec::new();

    for (root, relocation) in &relocations {
        if !relocation.is_retained {
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

                    let is_matching = identity == *module;
                    overlays.push((root.clone(), identity, previous));

                    if is_matching {
                        Ok(())
                    } else {
                        Err(Error::content_modified())
                    }
                }

                Err(error) => Err(error),
            };

            if let Err(error) = result {
                state.restore(overlays)?;

                return Err(error);
            }
        }
    }

    let mut replacements = Vec::new();
    let mut retired = Vec::new();

    for (root, relocation) in relocations {
        let mut project = state
            .projects
            .remove(&root)
            .expect("Internal error: renamed project is missing.");

        project.workspace = relocation.workspace;
        let previous = mem::replace(&mut project.generators, relocation.generators);

        if !relocation.is_retained {
            retired.push(previous);
            project.borrowed.clear();
        }

        if relocation.is_retained {
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
    let mut outcome = state.reconcile();

    for generators in retired {
        outcome = outcome.and(generators.shutdown().map_err(failure));
    }

    outcome?;

    Ok(cleared.into_iter().collect())
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
        let is_moved = future_module != Path::new(&module);
        let mut imports: BTreeMap<(usize, usize), BTreeSet<(String, String)>> = BTreeMap::new();

        for import in frontend.imports(&module).map_err(failure)? {
            if import.location.module != module || !Path::new(&import.target).is_absolute() {
                return Err(Error::content_modified());
            }

            let range = document.native_bytes(import.location.range)?;

            imports
                .entry((range.start, range.end))
                .or_default()
                .insert((import.target, import.identity));
        }

        if project.workspace.snapshot(&module).as_deref() != Some(document.text.as_ref()) {
            return Err(Error::content_modified());
        }

        if is_moved
            && document.analysis().imports.iter().any(|import| {
                !imports
                    .keys()
                    .any(|(start, end)| *start <= import.range.start && import.range.end <= *end)
            })
        {
            return Err(reason(
                "An unresolved import in the moved source cannot be updated safely.",
            ));
        }

        let mut edits = Vec::new();

        for ((start, end), targets) in imports {
            if !is_moved
                && targets
                    .iter()
                    .all(|(target, _)| relocated(Path::new(target), &mappings) == Path::new(target))
            {
                continue;
            }

            if targets.len() != 1 {
                return Err(reason(
                    "An import with multiple runtime targets cannot be updated safely.",
                ));
            }

            let (target, identity) = targets
                .into_iter()
                .next()
                .expect("Internal error: import target is missing.");

            let future_target = relocated(Path::new(&target), &mappings);

            let mut styles = document
                .analysis()
                .imports
                .iter()
                .filter(|import| start <= import.range.start && import.range.end <= end);

            let style = styles
                .next()
                .ok_or_else(|| reason("Unsupported import expression cannot be updated safely."))?;

            if styles.next().is_some() {
                return Err(reason(
                    "Overlapping import expressions cannot be updated safely.",
                ));
            }

            let is_instance = matches!(style.expression, ImportExpression::Instance(_));

            if matches!(&style.expression, ImportExpression::Path(_)) && identity != target {
                return Err(reason(
                    "An instance-backed string import cannot be updated safely before its sourcemap is updated.",
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
                .import(&future_module, &future_target, is_instance)
                .map_err(failure)?
                .ok_or_else(|| {
                    reason(if is_instance {
                        "Cannot generate an unambiguous instance import for the renamed source."
                    } else {
                        "Cannot generate an unambiguous import for the renamed source."
                    })
                })?;

            if is_instance && !is_unshadowed(&document, &replacement) {
                return Err(reason(
                    "Renamed instance import would use a shadowed or written global.",
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
            return Err(Error::invalid_params("Renamed import edits overlap."));
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

fn is_affected(project: &Project, paths: &[PathBuf]) -> Result<bool> {
    if paths.iter().any(|path| {
        is_intersecting(project.workspace.directory(), path)
            && (path
                .extension()
                .is_some_and(|extension| extension == "luau" || extension == "lua")
                || is_configuration(path)
                || path.is_dir()
                || project.workspace.directory().starts_with(path))
    }) {
        return Ok(true);
    }

    for dependency in project.dependencies() {
        let dependency = identity(&dependency)?;

        if paths.iter().any(|path| is_intersecting(&dependency, path)) {
            return Ok(true);
        }
    }

    Ok(false)
}

fn is_unchanged(project: &Project, path: &Path) -> Result<bool> {
    for dependency in project.reload_dependencies() {
        if is_intersecting(&identity(&dependency)?, path) {
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

fn mappings(parameters: RenameFilesParams, is_pending: bool) -> Result<Vec<(PathBuf, PathBuf)>> {
    let mut mappings = Vec::new();

    for file in parameters.files {
        let old = path(&file.old_uri)?;

        let old = if is_pending {
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
                "A source cannot be moved inside itself.",
            ));
        }

        mappings.push((old, new));
    }

    for (index, (old, new)) in mappings.iter().enumerate() {
        if mappings[..index]
            .iter()
            .any(|(previous_old, previous_new)| {
                is_intersecting(old, previous_old) || is_intersecting(new, previous_new)
            })
        {
            return Err(Error::invalid_params(
                "File rename mappings overlap or collide.",
            ));
        }

        if is_pending
            && new.try_exists().map_err(failure)?
            && !mappings.iter().any(|(source, _)| new.starts_with(source))
            && identity(new)? != identity(old)?
        {
            return Err(Error::invalid_params(
                "File rename destination already exists.",
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

fn path(uri: &Uri) -> Result<PathBuf> {
    if uri.query().is_some() || uri.fragment().is_some() {
        return Err(Error::invalid_params(
            "File operation URIs cannot contain queries or fragments.",
        ));
    }

    document::path(uri)
}

fn is_unshadowed(document: &Document, expression: &str) -> bool {
    let root = expression.split(['.', ':', '[']).next().unwrap_or_default();

    matches!(root, "script" | "game" | "workspace")
        && !document
            .analysis()
            .bindings
            .iter()
            .any(|binding| binding.name == root)
        && !document.analysis().tokens.iter().any(|token| {
            token.access.is_write() && document.text.get(token.range.clone()) == Some(root)
        })
}
