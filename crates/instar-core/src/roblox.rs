//! Sourcemap-backed Roblox instance identities and navigation.

use std::{
    collections::{BTreeSet, HashMap},
    fs,
    hash::{Hash, Hasher},
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeStruct};

use crate::{
    absolute, invalid,
    resolve::{Failure, Module, module_path},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawNode {
    name: String,
    class_name: String,

    #[serde(default)]
    file_paths: Vec<String>,

    #[serde(default)]
    children: Vec<Self>,
}

#[derive(Debug)]
struct Node {
    name: String,
    class_name: String,
    full_name: String,
    parent: Option<usize>,
    children: HashMap<String, Vec<usize>>,
    sources: Vec<PathBuf>,
}

/// A sourcemap location and the file that selected it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SourcemapLocation {
    /// Absolute path to the sourcemap JSON file.
    pub path: PathBuf,

    /// Absolute path to the defining `instar.toml`, or the map itself when auto-discovered.
    pub defined_in: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Sourcemap {
    pub(crate) path: PathBuf,
    nodes: Vec<Node>,
    sources: HashMap<PathBuf, Vec<usize>>,
}

impl Sourcemap {
    pub(crate) fn load(path: PathBuf) -> io::Result<Rc<Self>> {
        let root: Option<RawNode> = serde_json::from_slice(&fs::read(&path)?)?;
        let root = root.ok_or_else(|| invalid("sourcemap contains no root instance"))?;

        let base = path
            .parent()
            .ok_or_else(|| invalid("sourcemap has no directory"))?;

        let mut nodes: Vec<Node> = Vec::new();
        let mut sources: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        let mut pending: Vec<(RawNode, Option<usize>)> = vec![(root, None)];

        while let Some((raw, parent)) = pending.pop() {
            if raw.class_name.is_empty() || raw.class_name.contains('\0') || raw.name.contains('\0')
            {
                return Err(invalid("invalid sourcemap instance name or class"));
            }

            let id = nodes.len();

            let full_name = if let Some(parent) = parent {
                format!("{}.{}", nodes[parent].full_name, raw.name)
            } else if raw.class_name == "DataModel" {
                "game".to_owned()
            } else {
                raw.name.clone()
            };

            let mut files = BTreeSet::new();

            if matches!(
                raw.class_name.as_str(),
                "Script" | "LocalScript" | "ModuleScript"
            ) {
                for file in raw.file_paths {
                    if file.contains('\0') {
                        return Err(invalid("sourcemap file path contains NUL"));
                    }

                    let path = PathBuf::from(file.replace('\\', "/"));

                    if matches!(
                        path.extension().and_then(|s| s.to_str()),
                        Some("lua" | "luau")
                    ) {
                        files.insert(absolute(&base.join(path))?);
                    }
                }
            }

            for source in &files {
                sources.entry(source.clone()).or_default().push(id);
            }

            if let Some(parent) = parent {
                nodes[parent]
                    .children
                    .entry(raw.name.clone())
                    .or_default()
                    .push(id);
            }

            nodes.push(Node {
                name: raw.name,
                class_name: raw.class_name,
                full_name,
                parent,
                children: HashMap::new(),
                sources: files.into_iter().collect(),
            });

            pending.extend(
                raw.children
                    .into_iter()
                    .rev()
                    .map(|child| (child, Some(id))),
            );
        }

        Ok(Rc::new(Self {
            path,
            nodes,
            sources,
        }))
    }

    pub(crate) fn register(
        self: &Rc<Self>,
        checker: &mut instar_bridge::Checker,
        mut module_name: impl FnMut(Module) -> io::Result<String>,
    ) -> io::Result<()> {
        let names = self
            .nodes
            .iter()
            .enumerate()
            .map(|(index, node)| {
                if node.sources.is_empty() {
                    return Ok(None);
                }

                let module = self
                    .instance(index)
                    .module(false)
                    .map_err(|error| invalid(error.to_string()))?;

                module_name(module).map(Some)
            })
            .collect::<io::Result<Vec<_>>>()?;

        let nodes = self
            .nodes
            .iter()
            .zip(&names)
            .map(|(node, module)| instar_bridge::RobloxNode {
                name: &node.name,
                class_name: &node.class_name,
                parent: node.parent,
                module: module.as_deref(),
            })
            .collect::<Vec<_>>();

        checker.register_roblox_tree(&nodes)
    }

    pub(crate) fn instances_for_source(
        self: &Rc<Self>,
        path: &Path,
    ) -> impl Iterator<Item = Instance> + '_ {
        self.sources
            .get(path)
            .into_iter()
            .flatten()
            .map(|&node| self.instance(node))
    }

    pub(crate) fn find_source(self: &Rc<Self>, path: &Path) -> Result<Option<Instance>, Failure> {
        match self.sources.get(path).map_or(&[][..], Vec::as_slice) {
            [] => Ok(None),
            &[id] => Ok(Some(self.instance(id))),

            ids => Err(Failure::Roblox(format!(
                "source {} maps to multiple instances: {}",
                path.display(),
                ids.iter()
                    .map(|&id| self.nodes[id].full_name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    pub(crate) fn game(self: &Rc<Self>) -> Result<Instance, Failure> {
        if self.nodes[0].class_name != "DataModel" {
            return Err(Failure::Roblox(
                "sourcemap root is not a DataModel; game is unavailable".into(),
            ));
        }

        Ok(self.instance(0))
    }

    fn instance(self: &Rc<Self>, node: usize) -> Instance {
        Instance {
            map: Rc::clone(self),
            node,
        }
    }
}

/// An instance identity in one immutable sourcemap snapshot.
/// Different instances remain distinct even when they share a source file.
#[derive(Clone, Debug)]
pub struct Instance {
    pub(crate) map: Rc<Sourcemap>,
    node: usize,
}

impl PartialEq for Instance {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.map, &other.map) && self.node == other.node
    }
}

impl Eq for Instance {}

impl Hash for Instance {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::ptr::hash(Rc::as_ptr(&self.map), state);
        self.node.hash(state);
    }
}

impl Serialize for Instance {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = serializer.serialize_struct("Instance", 3)?;
        value.serialize_field("sourcemap", &self.map.path)?;
        value.serialize_field("node", &self.node)?;
        value.serialize_field("name", self.full_name())?;

        value.end()
    }
}

impl Instance {
    /// Absolute sourcemap path identifying this place context.
    #[must_use]
    pub fn sourcemap_path(&self) -> &Path {
        &self.map.path
    }

    /// Instance name, as recorded by the sourcemap.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.map.nodes[self.node].name
    }

    /// Engine class name, as recorded by the sourcemap.
    #[must_use]
    pub fn class_name(&self) -> &str {
        &self.map.nodes[self.node].class_name
    }

    /// Human-readable hierarchy path, rooted at `game` for a data model.
    #[must_use]
    pub fn full_name(&self) -> &str {
        &self.map.nodes[self.node].full_name
    }

    pub(crate) fn parent(&self) -> Result<Self, Failure> {
        self.map.nodes[self.node]
            .parent
            .map(|id| self.map.instance(id))
            .ok_or_else(|| Failure::Roblox(format!("{} has no mapped parent", self.full_name())))
    }

    pub(crate) fn child(&self, name: &str) -> Result<Self, Failure> {
        let matches = self.map.nodes[self.node]
            .children
            .get(name)
            .map_or(&[][..], Vec::as_slice);

        self.unique(matches.iter().copied(), &format!("child {name:?}"))
    }

    pub(crate) fn child_names(&self) -> impl Iterator<Item = &str> {
        self.map.nodes[self.node]
            .children
            .keys()
            .map(String::as_str)
    }

    pub(crate) fn find_child(&self, name: &str, recursive: bool) -> Result<Self, Failure> {
        if !recursive {
            return self.child(name);
        }

        let mut pending = vec![self.node];
        let mut found = Vec::new();

        while let Some(id) = pending.pop() {
            for &child in self.map.nodes[id].children.values().flatten() {
                if self.map.nodes[child].name == name {
                    found.push(child);
                }

                pending.push(child);
            }
        }

        self.unique(found.into_iter(), &format!("descendant {name:?}"))
    }

    pub(crate) fn service(&self, class: &str) -> Result<Self, Failure> {
        if self.class_name() != "DataModel" {
            return Err(Failure::Roblox(format!(
                "GetService requires game, not {}",
                self.full_name()
            )));
        }

        let matches = self.map.nodes[self.node]
            .children
            .values()
            .flatten()
            .copied()
            .filter(|&id| self.map.nodes[id].class_name == class);

        self.unique(matches, &format!("service {class:?}"))
    }

    fn unique(
        &self,
        mut matches: impl Iterator<Item = usize>,
        description: &str,
    ) -> Result<Self, Failure> {
        let id = matches.next().ok_or_else(|| {
            Failure::Roblox(format!("{} has no mapped {description}", self.full_name()))
        })?;

        if matches.next().is_some() {
            return Err(Failure::Roblox(format!(
                "ambiguous {description} under {}",
                self.full_name()
            )));
        }

        Ok(self.map.instance(id))
    }

    pub(crate) fn walk(&self, path: &str) -> Result<Self, Failure> {
        let mut current = self.clone();

        for component in path.split('/') {
            current = match component {
                "" | "." => current,
                ".." => current.parent()?,
                name => current.child(name)?,
            };
        }

        Ok(current)
    }

    pub(crate) fn module(&self, requiring: bool) -> Result<Module, Failure> {
        if requiring && self.class_name() != "ModuleScript" {
            return Err(Failure::Roblox(format!(
                "{} is a {}, not a ModuleScript",
                self.full_name(),
                self.class_name()
            )));
        }

        match self.map.nodes[self.node].sources.as_slice() {
            [source] => Ok(Module {
                path: module_path(source),
                source: source.clone(),
                instance: Some(self.clone()),
            }),

            [] => Err(Failure::Roblox(format!(
                "{} has no mapped Luau source",
                self.full_name()
            ))),

            _ => Err(Failure::Roblox(format!(
                "{} maps to multiple Luau sources",
                self.full_name()
            ))),
        }
    }
}
