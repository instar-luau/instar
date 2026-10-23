use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    error::Error as _,
    path::{Path, PathBuf},
    sync::Arc,
};

use ignore::WalkBuilder;
use instar_bridge::{Instance, Location};
use serde::Deserialize;

use super::{Error, Workspace, model::Scope, paths};
use crate::settings::resolution::Generator;

impl Workspace {
    /// Return whether Roblox instance support is enabled for a module.
    ///
    /// # Errors
    /// Returns inaccessible configuration or sourcemap discovery paths.
    pub fn is_roblox(&self, module: &str) -> Result<bool, Error> {
        let module = Path::new(module);
        let scope = self.scope(module)?;

        is_enabled(self, &scope, module)
    }
}

#[derive(Default)]
pub(super) struct Registry {
    automatic: BTreeMap<PathBuf, Option<PathBuf>>,
    projects: Option<Vec<Generator>>,
    inputs: BTreeMap<(PathBuf, PathBuf), Option<usize>>,
    generated: BTreeMap<Generator, Arc<Sourcemap>>,
    generators: BTreeMap<Generator, Option<usize>>,
    maps: Vec<Map>,
    rojo: Option<bool>,
}

impl Registry {
    pub(super) fn generated(&self) -> BTreeMap<Generator, Arc<Sourcemap>> {
        self.generated.clone()
    }

    pub(super) fn discovered(&self) -> impl Iterator<Item = &Generator> {
        self.projects
            .iter()
            .flatten()
            .filter(|_| self.rojo != Some(false))
    }

    pub(super) fn set_generated(&mut self, generated: BTreeMap<Generator, Arc<Sourcemap>>) {
        *self = Self {
            generated,
            ..Self::default()
        };
    }

    pub(super) fn dependencies(&self, dependencies: &mut BTreeSet<PathBuf>) {
        dependencies.extend(
            self.automatic
                .keys()
                .map(|directory| directory.join("sourcemap.json")),
        );

        dependencies.extend(self.automatic.values().flatten().cloned());
        dependencies.extend(self.inputs.keys().map(|(path, _)| path.clone()));

        dependencies.extend(
            self.maps
                .iter()
                .flat_map(|map| map.references.iter().cloned()),
        );

        dependencies.extend(
            self.generators
                .iter()
                .filter(|(_, index)| index.is_none())
                .map(|(generator, _)| generator.directory.clone()),
        );
    }

    pub(super) fn get(&self, name: &str) -> Option<&[Instance]> {
        self.maps
            .get(name.parse::<usize>().ok()?)
            .map(|map| map.instances.as_slice())
    }

    fn project(&mut self, workspace: &Workspace) -> Result<Vec<Generator>, Error> {
        if self.projects.is_none() {
            self.projects = Some(discover(workspace)?);
        }

        let found = self
            .projects
            .as_ref()
            .expect("Internal error: project discovery is missing.");

        if !found.is_empty() && self.rojo.is_none() {
            let available = env::var_os("PATH").is_some_and(|path| {
                env::split_paths(&path).any(|directory| {
                    directory
                        .join(format!("rojo{}", env::consts::EXE_SUFFIX))
                        .is_file()
                })
            });

            self.rojo = Some(available);

            if !available {
                tracing::warn!(
                    "Rojo was not found on PATH; automatic sourcemap generation is unavailable."
                );
            }
        }

        Ok(found.clone())
    }

    fn automatic(
        &mut self,
        workspace: &Workspace,
        directory: &Path,
    ) -> Result<Option<PathBuf>, Error> {
        let mut pending = Vec::new();
        let mut found = None;

        for ancestor in directory.ancestors() {
            if let Some(value) = self.automatic.get(ancestor) {
                found.clone_from(value);
                break;
            }

            pending.push(ancestor.to_owned());
            let path = ancestor.join("sourcemap.json");

            if paths::metadata(&path)?.is_some_and(|metadata| metadata.is_file()) {
                found = Some(paths::canonical(&path)?);
                break;
            }
        }

        if !pending.is_empty() {
            workspace.observations.borrow_mut().take();
        }

        for directory in pending {
            self.automatic.insert(directory, found.clone());
        }

        Ok(found)
    }

    fn load(&mut self, workspace: &Workspace, path: &Path, base: &Path) -> Option<usize> {
        let key = (path.to_owned(), base.to_owned());

        if let Some(index) = self.inputs.get(&key) {
            return *index;
        }

        workspace.observations.borrow_mut().take();

        let result = (|| {
            let root: Option<Node> =
                serde_json::from_str(&paths::read(path)?).map_err(|source| Error::Data {
                    path: path.to_owned(),
                    source,
                })?;

            let map = Map::new(root.as_ref(), base)?;
            let index = self.maps.len();
            self.maps.push(map);

            Ok::<_, Error>(index)
        })();

        let index = match result {
            Ok(index) => Some(index),

            Err(error) => {
                tracing::warn!(path = %path.display(), %error, cause = error.source().map(ToString::to_string), "Could not load sourcemap");

                None
            }
        };

        self.inputs.insert(key, index);

        index
    }

    fn generate(
        &mut self,
        workspace: &Workspace,
        generator: &Generator,
    ) -> Result<Option<usize>, Error> {
        if let Some(index) = self.generators.get(generator) {
            return Ok(*index);
        }

        workspace.observations.borrow_mut().take();

        let result = (|| {
            let source = if let Some(source) = self.generated.get(generator) {
                Arc::clone(source)
            } else {
                let source = workspace.generate(generator)?;

                self.generated
                    .insert(generator.clone(), Arc::clone(&source));

                source
            };

            let map = Map::new(source.root.as_ref(), &generator.directory)?;
            let index = self.maps.len();
            self.maps.push(map);

            Ok(Some(index))
        })();

        if matches!(result, Err(Error::Cancelled)) {
            return result;
        }

        let index = result.as_ref().ok().copied().flatten();

        if let Err(error) = &result
            && (workspace.generation.is_none() || generator.watch.is_empty())
        {
            tracing::warn!(directory = %generator.directory.display(), %error, cause = error.source().map(ToString::to_string), "Sourcemap generation failed");
        }

        self.generators.insert(generator.clone(), index);

        Ok(index)
    }
}

struct Map {
    instances: Vec<Instance>,
    modules: BTreeMap<String, Vec<usize>>,
    references: BTreeSet<PathBuf>,
}

impl Map {
    fn new(root: Option<&Node>, base: &Path) -> Result<Self, Error> {
        let mut map = Self {
            instances: Vec::new(),
            modules: BTreeMap::new(),
            references: BTreeSet::new(),
        };

        if let Some(root) = root {
            map.insert(root, None, base)?;
        }

        Ok(map)
    }

    fn insert(&mut self, node: &Node, parent: Option<usize>, base: &Path) -> Result<(), Error> {
        let index = self.instances.len();
        let mut sources = BTreeSet::new();

        for path in &node.file_paths {
            let path = base.join(path);

            if paths::is_source(&path) {
                let path = if paths::metadata(&path)?.is_some() {
                    paths::canonical(&path)?
                } else if path.parent().is_some_and(Path::is_dir) {
                    paths::identity(&path)?
                } else {
                    path
                };

                sources.insert(paths::text(&path)?);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(".project.json"))
            {
                let path = if paths::metadata(&path)?.is_some() {
                    paths::canonical(&path)?
                } else {
                    path
                };

                self.references.insert(path);
            }
        }

        for source in &sources {
            self.modules.entry(source.clone()).or_default().push(index);
        }

        self.instances.push(Instance {
            name: node.name.clone(),
            class: node.class_name.clone(),
            parent,
            module: (sources.len() == 1)
                .then(|| sources.into_iter().next())
                .flatten(),
        });

        for child in &node.children {
            self.insert(child, Some(index), base)?;
        }

        Ok(())
    }
}

/// Parsed sourcemap shared by generator sessions and workspace workers.
#[derive(Deserialize, Eq, PartialEq)]
#[serde(transparent)]
pub struct Sourcemap {
    root: Option<Node>,
}

#[derive(Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Node {
    name: String,
    class_name: String,

    #[serde(default)]
    file_paths: Vec<PathBuf>,

    #[serde(default)]
    children: Vec<Self>,
}

pub(super) fn is_enabled(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<bool, Error> {
    if scope.configuration.environment.is_roblox == Some(false) {
        return Ok(false);
    }

    Ok(!generators(workspace, scope)?.is_empty()
        || scope.configuration.environment.is_roblox == Some(true)
        || !inputs(workspace, scope, module)?.is_empty())
}

pub(super) fn configuration(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<Vec<Location>, Error> {
    if !is_enabled(workspace, scope, module)? {
        return Ok(Vec::new());
    }

    let inputs = inputs(workspace, scope, module)?;
    let generators = generators(workspace, scope)?;
    let mut registry = workspace.instances.borrow_mut();
    let mut indices = Vec::new();
    let mut projects = BTreeMap::new();
    let mut references = BTreeSet::new();

    if scope.configuration.resolve.generator.is_some() || registry.rojo != Some(false) {
        for generator in generators {
            let Some(index) = registry.generate(workspace, &generator)? else {
                continue;
            };

            if scope.configuration.resolve.generator.is_none() {
                let project = paths::canonical(Path::new(&generator.command[2]))?;

                references.extend(
                    registry.maps[index]
                        .references
                        .iter()
                        .filter(|reference| **reference != project)
                        .cloned(),
                );

                projects.insert(index, project);
            }

            indices.push(index);
        }
    }

    for (path, base) in inputs {
        indices.extend(registry.load(workspace, &path, &base));
    }

    let mut maps = BTreeSet::new();
    let mut locations = Vec::new();
    let module_name = paths::text(module)?;

    for index in indices {
        if projects
            .get(&index)
            .is_some_and(|project| references.contains(project))
        {
            continue;
        }

        if !maps.insert(index) {
            continue;
        }

        if let Some(instances) = registry.maps[index].modules.get(&module_name) {
            locations.extend(instances.iter().map(|instance| Location {
                hierarchy: index.to_string(),
                script: Some(*instance),
            }));
        }
    }

    if locations.is_empty()
        && registry.generators.values().all(Option::is_some)
        && registry.inputs.values().all(Option::is_some)
        && let Some(map) = maps.first().filter(|_| maps.len() == 1)
    {
        locations.push(Location {
            hierarchy: map.to_string(),
            script: None,
        });
    }

    Ok(locations)
}

fn inputs(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<Vec<(PathBuf, PathBuf)>, Error> {
    if let Some(paths) = &scope.configuration.resolve.sourcemaps {
        return Ok(paths
            .iter()
            .map(|path| (path.clone(), workspace.root.clone()))
            .collect());
    }

    if !generators(workspace, scope)?.is_empty()
        && (scope.configuration.resolve.generator.is_some()
            || workspace.instances.borrow().rojo != Some(false))
    {
        return Ok(Vec::new());
    }

    Ok(workspace
        .instances
        .borrow_mut()
        .automatic(workspace, module.parent().unwrap_or(&workspace.root))?
        .map(|path| {
            let base = path.parent().unwrap_or(&workspace.root).to_owned();

            (path, base)
        })
        .into_iter()
        .collect())
}

fn generators(workspace: &Workspace, scope: &Scope) -> Result<Vec<Generator>, Error> {
    if let Some(generator) = &scope.configuration.resolve.generator {
        return Ok((!generator.command.is_empty())
            .then(|| generator.clone())
            .into_iter()
            .collect());
    }

    if scope.configuration.resolve.sourcemaps.is_some() {
        return Ok(Vec::new());
    }

    workspace.instances.borrow_mut().project(workspace)
}

fn discover(workspace: &Workspace) -> Result<Vec<Generator>, Error> {
    let mut projects = BTreeSet::new();
    let cancellation = workspace.cancellation.borrow().clone();
    let is_cancelled = &|| cancellation.is_cancelled();

    for entry in WalkBuilder::new(&workspace.root)
        .standard_filters(false)
        .build()
    {
        super::process::interrupted(is_cancelled)?;
        let entry = entry.map_err(Error::Traversal)?;

        let path = entry.path();

        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".project.json"))
            && entry.file_type().is_some_and(|kind| kind.is_file())
            && workspace
                .scope(path)?
                .configuration
                .exclude
                .as_ref()
                .is_none_or(|exclude| !exclude.is_match(path, false))
        {
            let path = paths::canonical(path)?;
            workspace.observations.borrow_mut().take();

            workspace
                .configuration_paths
                .borrow_mut()
                .insert(path.clone());

            projects.insert(path);
        }
    }

    projects
        .into_iter()
        .map(|project| {
            Ok(Generator {
                command: vec![
                    "rojo".to_owned(),
                    "sourcemap".to_owned(),
                    paths::text(&project)?,
                    "--include-non-scripts".to_owned(),
                ],
                watch: vec!["--watch".to_owned()],
                directory: project
                    .parent()
                    .expect("Internal error: project has no parent directory.")
                    .to_owned(),
            })
        })
        .collect()
}
