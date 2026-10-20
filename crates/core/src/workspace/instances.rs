use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Arc,
};

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
    inputs: BTreeMap<(PathBuf, PathBuf), usize>,
    generated: BTreeMap<Generator, Arc<str>>,
    generators: BTreeMap<Generator, usize>,
    maps: Vec<Map>,
}

impl Registry {
    pub(super) fn generated(&self) -> BTreeMap<Generator, Arc<str>> {
        self.generated.clone()
    }

    pub(super) fn set_generated(&mut self, generated: BTreeMap<Generator, Arc<str>>) {
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
    }

    pub(super) fn get(&self, name: &str) -> Option<Vec<Instance>> {
        self.maps
            .get(name.parse::<usize>().ok()?)
            .map(|map| map.instances.clone())
    }

    fn automatic(&mut self, directory: &Path) -> Result<Option<PathBuf>, Error> {
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

        for directory in pending {
            self.automatic.insert(directory, found.clone());
        }

        Ok(found)
    }

    fn load(&mut self, path: &Path, base: &Path) -> Result<usize, Error> {
        let path = paths::canonical(path)?;
        let key = (path.clone(), base.to_owned());

        if let Some(index) = self.inputs.get(&key) {
            return Ok(*index);
        }

        let root: Option<Node> =
            serde_json::from_str(&paths::read(&path)?).map_err(|source| Error::Data {
                path: path.clone(),
                source,
            })?;

        let map = Map::new(root, base)?;
        let index = self.maps.len();
        self.maps.push(map);
        self.inputs.insert(key, index);

        Ok(index)
    }

    fn generate(&mut self, workspace: &Workspace, generator: &Generator) -> Result<usize, Error> {
        if let Some(index) = self.generators.get(generator) {
            return Ok(*index);
        }

        let source = if let Some(source) = self.generated.get(generator) {
            Arc::clone(source)
        } else {
            let source = workspace.generate(generator)?;

            self.generated
                .insert(generator.clone(), Arc::clone(&source));

            source
        };

        let root = serde_json::from_str(&source).map_err(|source| Error::GeneratedData {
            executable: generator.command.first().cloned().unwrap_or_default(),
            source,
        })?;

        let map = Map::new(root, &generator.directory)?;
        let index = self.maps.len();
        self.maps.push(map);
        self.generators.insert(generator.clone(), index);

        Ok(index)
    }
}

struct Map {
    instances: Vec<Instance>,
    modules: BTreeMap<String, Vec<usize>>,
}

impl Map {
    fn new(root: Option<Node>, base: &Path) -> Result<Self, Error> {
        let mut map = Self {
            instances: Vec::new(),
            modules: BTreeMap::new(),
        };

        if let Some(root) = root {
            map.insert(root, None, base)?;
        }

        Ok(map)
    }

    fn insert(&mut self, node: Node, parent: Option<usize>, base: &Path) -> Result<(), Error> {
        let index = self.instances.len();
        let mut sources = BTreeSet::new();

        for path in node.file_paths {
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
            }
        }

        for source in &sources {
            self.modules.entry(source.clone()).or_default().push(index);
        }

        self.instances.push(Instance {
            name: node.name,
            class: node.class_name,
            parent,
            module: (sources.len() == 1)
                .then(|| sources.into_iter().next())
                .flatten(),
        });

        for child in node.children {
            self.insert(child, Some(index), base)?;
        }

        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node {
    name: String,
    class_name: String,

    #[serde(default)]
    file_paths: Vec<PathBuf>,

    #[serde(default)]
    children: Vec<Self>,
}

pub(super) fn validate(source: &str) -> Result<(), serde_json::Error> {
    let _root: Option<Node> = serde_json::from_str(source)?;

    Ok(())
}

pub(super) fn is_enabled(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<bool, Error> {
    if let Some(is_enabled) = scope.configuration.environment.is_roblox {
        return Ok(is_enabled);
    }

    Ok(generator(scope).is_some() || !inputs(workspace, scope, module)?.is_empty())
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
    let mut registry = workspace.instances.borrow_mut();
    let mut indices = Vec::new();

    if let Some(generator) = generator(scope) {
        indices.push(registry.generate(workspace, generator)?);
    }

    for (path, base) in inputs {
        indices.push(registry.load(&path, &base)?);
    }

    let mut maps = BTreeSet::new();
    let mut locations = Vec::new();
    let module_name = paths::text(module)?;

    for index in indices {
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

    if generator(scope).is_some() {
        return Ok(Vec::new());
    }

    Ok(workspace
        .instances
        .borrow_mut()
        .automatic(module.parent().unwrap_or(&workspace.root))?
        .map(|path| {
            let base = path.parent().unwrap_or(&workspace.root).to_owned();

            (path, base)
        })
        .into_iter()
        .collect())
}

fn generator(scope: &Scope) -> Option<&Generator> {
    scope
        .configuration
        .resolve
        .generator
        .as_ref()
        .filter(|generator| !generator.command.is_empty())
}
