mod cache;

use crate::{
    project::configuration::{RobloxConfig, RobloxLevel},
    project::resolution::Resolver,
    source::absolute,
};

use serde::Deserialize;
use serde_json::Value;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Node {
    pub name: String,
    pub class_name: String,

    #[serde(default)]
    file_paths: Vec<PathBuf>,

    #[serde(default)]
    children: Vec<Self>,

    #[serde(skip)]
    pub parent: Option<usize>,

    #[serde(skip)]
    pub descendants: Vec<usize>,
}

#[derive(Default)]
pub(crate) struct Environment {
    pub enabled: bool,
    pub definitions: String,
    pub documentation: std::sync::Arc<crate::analysis::Documentation>,
    pub nodes: Vec<Node>,
    pub enumerations: Vec<String>,
    pub classes: Vec<String>,
    origin: PathBuf,
    files: BTreeMap<PathBuf, usize>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[derive(Deserialize)]
struct Metadata {
    classes: Vec<Class>,
    enumerations: Vec<String>,
}

#[derive(Deserialize)]
struct Class {
    name: String,
    service: bool,
    creatable: bool,
}

impl Environment {
    pub(crate) fn load(
        resolver: &mut Resolver<'_>,
        configuration: &RobloxConfig,
        update: bool,
    ) -> io::Result<Self> {
        let project_cache = configuration.root.join(".instar/roblox");

        let directory = if project_cache.join("current.json").is_file() {
            project_cache
        } else {
            dirs::cache_dir()
                .map(|path| path.join("instar").join("roblox"))
                .ok_or_else(|| io::Error::other("cannot locate the Roblox cache directory"))?
        };

        let bundle = cache::load(&directory, update)?;

        let level = match configuration.level.unwrap_or_default() {
            RobloxLevel::None => "none",
            RobloxLevel::LocalUserSecurity => "local",
            RobloxLevel::PluginSecurity => "plugin",
            RobloxLevel::RobloxScriptSecurity => "roblox",
        };

        let source = bundle
            .definitions
            .get(level)
            .ok_or_else(|| invalid("missing Roblox level"))?;

        let (metadata, definitions) = cache::metadata(source)?;

        let mut environment = Self {
            enabled: true,
            definitions: definitions.to_owned(),
            documentation: std::sync::Arc::new(bundle.documentation),
            classes: metadata
                .classes
                .iter()
                .map(|class| {
                    format!(
                        "{}\0{}{}",
                        class.name,
                        u8::from(class.service),
                        u8::from(class.creatable)
                    )
                })
                .collect(),
            enumerations: metadata.enumerations.clone(),
            ..Self::default()
        };

        if let Some(path) = &configuration.sourcemap {
            let source = resolver.load(path)?;

            let node: Node = serde_json::from_slice(source.bytes())
                .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

            environment.origin.clone_from(path);

            environment.insert(
                node,
                None,
                configuration
                    .project
                    .as_ref()
                    .unwrap_or(path)
                    .parent()
                    .ok_or_else(|| invalid("sourcemap project has no parent"))?,
            )?;
        } else if let Some(path) = &configuration.project {
            let mut mapping = Mapping::default();
            let node = project(resolver, path, &mut mapping)?;
            environment.origin.clone_from(path);

            environment.insert(
                node,
                None,
                path.parent()
                    .ok_or_else(|| invalid("project has no parent"))?,
            )?;
        }

        Ok(environment)
    }

    fn insert(
        &mut self,
        mut node: Node,
        parent: Option<usize>,
        directory: &Path,
    ) -> io::Result<usize> {
        if node.name.contains('\0') || node.class_name.contains('\0') {
            return Err(invalid("instance names cannot contain NUL"));
        }

        let index = self.nodes.len();
        node.parent = parent;
        let children = std::mem::take(&mut node.children);

        node.file_paths = node
            .file_paths
            .into_iter()
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
            })
            .map(|path| absolute(&directory.join(path)).map_err(io::Error::other))
            .collect::<io::Result<_>>()?;

        if node.file_paths.len() > 1 {
            return Err(invalid(format!(
                "{} has multiple script sources",
                node.name
            )));
        }

        for path in &node.file_paths {
            if self.files.insert(path.clone(), index).is_some() {
                return Err(invalid(format!(
                    "{} maps to multiple instances",
                    path.display()
                )));
            }
        }

        self.nodes.push(node);
        let mut names = BTreeSet::new();

        for child in children {
            if !names.insert(child.name.clone()) {
                return Err(invalid(format!("ambiguous child {}", child.name)));
            }

            let child = self.insert(child, Some(index), directory)?;
            self.nodes[index].descendants.push(child);
        }

        Ok(index)
    }

    pub(crate) fn identity(&self, index: usize) -> PathBuf {
        self.origin.join(index.to_string())
    }

    pub(crate) fn node(&self, path: &Path) -> Option<usize> {
        self.files.get(path).copied().or_else(|| {
            (path.parent()? == self.origin)
                .then(|| path.file_name()?.to_str()?.parse::<usize>().ok())
                .flatten()
                .filter(|index| *index < self.nodes.len())
        })
    }

    pub(crate) fn readable(&self, path: &Path) -> bool {
        self.node(path)
            .is_none_or(|index| !self.nodes[index].file_paths.is_empty())
    }

    pub(crate) fn source(&self, path: &Path) -> PathBuf {
        self.node(path)
            .and_then(|index| self.nodes[index].file_paths.first())
            .cloned()
            .unwrap_or_else(|| path.to_owned())
    }

    pub(crate) fn configuration(&self, path: &Path) -> PathBuf {
        let source = self.source(path);

        if source.parent() == Some(&self.origin) {
            self.origin.clone()
        } else {
            source
        }
    }

    pub(crate) fn require(
        &self,
        resolver: &mut Resolver<'_>,
        from: &Path,
        specifier: &str,
    ) -> io::Result<Option<PathBuf>> {
        let physical = self.configuration(from);

        let configured = (specifier == "@game" || specifier.starts_with("@game/"))
            && resolver
                .discovery
                .alias_names(&physical)?
                .iter()
                .any(|name| name.eq_ignore_ascii_case("game"));

        if !configured && let Some(index) = self.namespace(from, specifier) {
            return Ok(Some(self.identity(index)));
        }

        resolver.resolve(&physical, specifier)
    }

    pub(crate) fn namespace(&self, from: &Path, specifier: &str) -> Option<usize> {
        let (root, tail) = specifier.split_once('/').unwrap_or((specifier, ""));

        let mut index = match root {
            "@game" => (self.nodes.first()?.class_name == "DataModel").then_some(0)?,
            "@self" => self.node(from)?,
            _ => return None,
        };

        for segment in tail
            .split('/')
            .filter(|segment| !segment.is_empty() && *segment != ".")
        {
            index = if segment == ".." {
                self.nodes[index].parent?
            } else {
                *self.nodes[index]
                    .descendants
                    .iter()
                    .find(|child| self.nodes[**child].name == segment)?
            };
        }

        Some(index)
    }
}

struct Rule {
    root: PathBuf,
    matcher: globset::GlobMatcher,
    ignored: bool,
}

#[derive(Default)]
struct Mapping {
    visited: BTreeSet<PathBuf>,
    rules: Vec<Rule>,
    legacy: Option<bool>,
}

impl Mapping {
    fn ignored(&self, path: &Path) -> bool {
        self.rules.iter().fold(false, |ignored, rule| {
            if path
                .strip_prefix(&rule.root)
                .is_ok_and(|path| rule.matcher.is_match(path))
            {
                rule.ignored
            } else {
                ignored
            }
        })
    }
}

fn project(resolver: &mut Resolver<'_>, path: &Path, mapping: &mut Mapping) -> io::Result<Node> {
    let path = absolute(path).map_err(io::Error::other)?;

    if !mapping.visited.insert(path.clone()) {
        return Err(invalid(format!("cyclic project {}", path.display())));
    }

    let source = resolver.load(&path)?;

    let value: Value = serde_json::from_slice(source.bytes())
        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

    let directory = path
        .parent()
        .ok_or_else(|| invalid("project has no parent"))?;

    if value
        .get("syncRules")
        .and_then(Value::as_array)
        .is_some_and(|rules| !rules.is_empty())
    {
        return Err(invalid(format!(
            "{}: custom synchronization rules require a sourcemap",
            path.display()
        )));
    }

    let count = mapping.rules.len();
    let legacy = mapping.legacy;

    if let Some(value) = value.get("emitLegacyScripts") {
        mapping.legacy = Some(
            value
                .as_bool()
                .ok_or_else(|| invalid("emitLegacyScripts must be a boolean"))?,
        );
    }

    if let Some(rules) = value.get("globIgnorePaths") {
        for pattern in rules
            .as_array()
            .ok_or_else(|| invalid("globIgnorePaths must be an array"))?
        {
            let pattern = pattern
                .as_str()
                .ok_or_else(|| invalid("ignore patterns must be strings"))?;

            let (pattern, ignored) = if let Some(pattern) = pattern.strip_prefix('!') {
                (pattern, false)
            } else if pattern.starts_with(r"\!") {
                (&pattern[1..], true)
            } else {
                (pattern, true)
            };

            mapping.rules.push(Rule {
                root: directory.to_owned(),
                matcher: globset::Glob::new(pattern)
                    .map_err(io::Error::other)?
                    .compile_matcher(),
                ignored,
            });
        }
    }

    let fallback = if path
        .file_name()
        .is_some_and(|name| name == "default.project.json")
    {
        directory.file_name().and_then(|name| name.to_str())
    } else {
        path.file_stem()
            .and_then(|name| name.to_str())
            .map(|name| name.strip_suffix(".project").unwrap_or(name))
    };

    let name = value
        .get("name")
        .and_then(Value::as_str)
        .or(fallback)
        .ok_or_else(|| invalid("project requires a name"))?;

    let result = tree(
        resolver,
        name,
        &value["tree"],
        directory,
        mapping,
        Node::default(),
    );

    mapping.visited.remove(&path);
    mapping.rules.truncate(count);
    mapping.legacy = legacy;

    result
}

fn tree(
    resolver: &mut Resolver<'_>,
    name: &str,
    value: &Value,
    directory: &Path,
    mapping: &mut Mapping,
    existing: Node,
) -> io::Result<Node> {
    let object = value
        .as_object()
        .ok_or_else(|| invalid("project tree must be an object"))?;

    let mut node = if let Some(path) = object.get("$path") {
        let optional = path.get("optional").and_then(Value::as_str);

        let path = directory.join(
            path.as_str()
                .or(optional)
                .ok_or_else(|| invalid("$path must be a string or optional path"))?,
        );

        match filesystem(resolver, &path, mapping) {
            Ok(node) => node,
            Err(error) if optional.is_some() && error.kind() == io::ErrorKind::NotFound => existing,
            Err(error) => return Err(error),
        }
    } else {
        existing
    };

    node.name = name.into();

    if let Some(class) = object.get("$className") {
        node.class_name = class
            .as_str()
            .ok_or_else(|| invalid("$className must be a string"))?
            .into();
    }

    if node.class_name.is_empty() {
        node.class_name = "Folder".into();
    }

    for (name, child) in object {
        if !name.starts_with('$') {
            let existing = node
                .children
                .iter()
                .position(|existing| existing.name == name.as_str())
                .map(|index| node.children.remove(index))
                .unwrap_or_default();

            node.children
                .push(tree(resolver, name, child, directory, mapping, existing)?);
        }
    }

    Ok(node)
}

fn configure(resolver: &mut Resolver<'_>, node: &mut Node, path: &Path) -> io::Result<()> {
    match fs::metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }

    let source = resolver.load(path)?;

    let value: Value = serde_json::from_slice(source.bytes())
        .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

    if let Some(class) = value.get("className") {
        if node.class_name != "Folder" {
            return Err(invalid("directory className requires a folder"));
        }

        node.class_name = class
            .as_str()
            .ok_or_else(|| invalid("className must be a string"))?
            .into();
    }

    Ok(())
}

fn filesystem(resolver: &mut Resolver<'_>, path: &Path, mapping: &mut Mapping) -> io::Result<Node> {
    let path = absolute(path).map_err(io::Error::other)?;

    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == io::ErrorKind::NotFound && resolver.is_file(&path)? => None,
        Err(error) => return Err(error),
    };

    if metadata
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err(invalid(format!(
            "project symlink is unsupported: {}",
            path.display()
        )));
    }

    if resolver.is_file(&path)? && path.to_string_lossy().ends_with(".project.json") {
        return project(resolver, &path, mapping);
    }

    let mut node = Node {
        name: path
            .file_stem()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("project path requires UTF-8"))?
            .into(),
        ..Default::default()
    };

    if metadata.as_ref().is_some_and(std::fs::Metadata::is_dir) {
        let project_path = path.join("default.project.json");

        if project_path.is_file() {
            return project(resolver, &project_path, mapping);
        }

        node.name = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("project path requires UTF-8"))?
            .into();

        node.class_name = "Folder".into();

        for path in resolver.entries(&path)? {
            if mapping.ignored(&path) {
                continue;
            }

            if path.is_dir()
                || path
                    .extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
            {
                let child = filesystem(resolver, &path, mapping)?;

                if child.name == "init" {
                    if !node.file_paths.is_empty() {
                        return Err(invalid("ambiguous project initializer"));
                    }

                    node.class_name = child.class_name;
                    node.file_paths = child.file_paths;
                } else {
                    node.children.push(child);
                }
            }
        }

        configure(resolver, &mut node, &path.join("init.meta.json"))?;
    } else {
        if !path
            .extension()
            .is_some_and(|extension| extension == "lua" || extension == "luau")
        {
            return Err(invalid(format!(
                "unsupported project source {}",
                path.display()
            )));
        }

        node.class_name = if let Some(name) = node
            .name
            .strip_suffix(".server")
            .or_else(|| node.name.strip_suffix(".plugin"))
        {
            let name = name.into();
            node.name = name;

            "Script"
        } else if let Some(name) = node.name.strip_suffix(".client") {
            let name = name.into();
            node.name = name;

            if mapping.legacy.unwrap_or(true) {
                "LocalScript"
            } else {
                "Script"
            }
        } else {
            "ModuleScript"
        }
        .into();

        node.file_paths.push(path);
    }

    Ok(node)
}
