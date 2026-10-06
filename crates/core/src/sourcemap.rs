//! Immutable sourcemap instance contexts and navigation.

use crate::{
    configuration::invalid,
    resolve::{Identity, Module, module_path},
    source::{Document, normalize},
};

use serde::Deserialize;

use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    name: String,
    class_name: String,

    #[serde(default)]
    file_paths: Vec<PathBuf>,

    #[serde(default)]
    children: Vec<Self>,
}

struct Node {
    name: String,
    class: String,
    parent: Option<usize>,
    children: Vec<usize>,
    sources: Vec<PathBuf>,
}

pub(crate) struct Map {
    pub(crate) path: PathBuf,
    revision: u64,
    nodes: Vec<Node>,
    sources: BTreeMap<PathBuf, Vec<usize>>,
}

impl Map {
    pub(crate) fn parse(path: &Path, document: &Document) -> io::Result<Rc<Self>> {
        let root: Raw = serde_json::from_str(&document.text)
            .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

        let directory = path
            .parent()
            .ok_or_else(|| invalid("sourcemap has no directory"))?;

        let mut map = Self {
            path: path.to_path_buf(),
            revision: document.revision,
            nodes: Vec::new(),
            sources: BTreeMap::new(),
        };

        let mut pending: Vec<(Raw, Option<usize>)> = vec![(root, None)];

        while let Some((raw, parent)) = pending.pop() {
            if raw.class_name.is_empty() || raw.name.contains('\0') || raw.class_name.contains('\0')
            {
                return Err(invalid(format!(
                    "{}: invalid instance name or class",
                    path.display()
                )));
            }

            let index = map.nodes.len();
            let mut sources = Vec::new();

            for source in raw.file_paths {
                let text = source
                    .to_str()
                    .ok_or_else(|| invalid("sourcemap paths must be UTF-8"))?;

                if text.is_empty() || text.contains('\0') {
                    return Err(invalid("invalid sourcemap source path"));
                }

                if matches!(
                    raw.class_name.as_str(),
                    "ModuleScript" | "Script" | "LocalScript"
                ) && matches!(
                    source.extension().and_then(|extension| extension.to_str()),
                    Some("lua" | "luau")
                ) {
                    let source = normalize(&directory.join(text.replace('\\', "/")));

                    if !sources.contains(&source) {
                        map.sources.entry(source.clone()).or_default().push(index);
                        sources.push(source);
                    }
                }
            }

            if let Some(parent) = parent {
                map.nodes[parent].children.push(index);
            }

            map.nodes.push(Node {
                name: raw.name,
                class: raw.class_name,
                parent,
                children: Vec::new(),
                sources,
            });

            pending.extend(
                raw.children
                    .into_iter()
                    .rev()
                    .map(|child| (child, Some(index))),
            );
        }

        Ok(Rc::new(map))
    }

    pub(crate) fn placements(&self, source: &Path) -> Vec<Identity> {
        self.sources
            .get(source)
            .into_iter()
            .flatten()
            .map(|&node| self.identity(node))
            .collect()
    }

    fn identity(&self, node: usize) -> Identity {
        Identity::Instance {
            map: self.path.clone(),
            revision: self.revision,
            node,
        }
    }

    pub(crate) fn index(&self, identity: &Identity) -> io::Result<usize> {
        if let Identity::Instance {
            map,
            revision,
            node,
        } = identity
            && map == &self.path
            && *revision == self.revision
            && *node < self.nodes.len()
        {
            return Ok(*node);
        }

        Err(invalid(
            "instance context no longer belongs to the current sourcemap",
        ))
    }

    pub(crate) fn root(&self) -> io::Result<usize> {
        if self.nodes[0].class == "DataModel" {
            Ok(0)
        } else {
            Err(invalid(
                "sourcemap root is not a DataModel; game is unavailable",
            ))
        }
    }

    pub(crate) fn parent(&self, node: usize) -> io::Result<usize> {
        self.nodes[node]
            .parent
            .ok_or_else(|| invalid("instance has no mapped parent"))
    }

    pub(crate) fn child(&self, node: usize, name: &str, recursive: bool) -> io::Result<usize> {
        let mut pending = self.nodes[node].children.clone();
        let mut found = Vec::new();

        while let Some(child) = pending.pop() {
            if self.nodes[child].name == name {
                found.push(child);
            }

            if recursive {
                pending.extend(&self.nodes[child].children);
            }
        }

        unique(&found, &format!("child {name:?}"))
    }

    pub(crate) fn service(&self, node: usize, class: &str) -> io::Result<usize> {
        if node != self.root()? {
            return Err(invalid("GetService requires game"));
        }

        let matches = self.nodes[node]
            .children
            .iter()
            .copied()
            .filter(|&child| self.nodes[child].class == class)
            .collect::<Vec<_>>();

        unique(&matches, &format!("service {class:?}"))
    }

    pub(crate) fn module(&self, node: usize, requiring: bool) -> io::Result<Module> {
        let instance = &self.nodes[node];

        if requiring && instance.class != "ModuleScript" {
            return Err(invalid(format!(
                "{} is a {}, not a ModuleScript",
                instance.name, instance.class
            )));
        }

        let source = match instance.sources.as_slice() {
            [source] => source.clone(),

            [] => {
                return Err(invalid(format!(
                    "{} has no mapped Luau source",
                    instance.name
                )));
            }

            _ => {
                return Err(invalid(format!(
                    "{} maps to multiple Luau sources",
                    instance.name
                )));
            }
        };

        if source
            .file_name()
            .is_some_and(|name| name == ".config.luau")
        {
            return Err(invalid(".config.luau is not an importable module"));
        }

        Ok(Module {
            identity: self.identity(node),
            path: module_path(&source),
            source,
        })
    }
}

fn unique(matches: &[usize], description: &str) -> io::Result<usize> {
    match matches {
        [node] => Ok(*node),
        [] => Err(invalid(format!("no mapped {description}"))),
        _ => Err(invalid(format!("ambiguous mapped {description}"))),
    }
}
