//! Static require resolution from lexical facts and explicitly owned project inputs.

use std::{
    collections::{BTreeMap, VecDeque},
    fs, io,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;
use text_size::TextRange;

use crate::{
    project::{ConfigFile, Project},
    semantics::{Namespace, Semantics},
    source::{Source, SourceError, SourceStore},
    syntax::{Parse, SyntaxKind as K, SyntaxNode, string_bytes},
};

#[derive(Debug)]
pub enum Resolution {
    Resolved(Arc<Source>),
    Missing(Vec<PathBuf>),
    Ambiguous(Vec<PathBuf>),
    Dynamic,
    ContextRequired,
    Unsupported(&'static str),
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error(transparent)]
    Source(#[from] SourceError),
    #[error("{path}: {source}")]
    Sourcemap {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid or case-colliding Instar alias: {0}")]
    Alias(String),
}

#[derive(Debug)]
pub struct Dependency {
    pub range: TextRange,
    pub resolution: Resolution,
}

#[derive(Debug)]
pub struct Module {
    pub semantics: Semantics,
    pub dependencies: Vec<Dependency>,
}

/// Snapshot of a traversal, including the exact configuration and mapping used.
#[derive(Debug)]
pub struct DependencyGraph {
    pub configurations: Vec<ConfigFile>,
    pub sourcemap: Option<Arc<Source>>,
    pub modules: BTreeMap<PathBuf, Module>,
}

impl DependencyGraph {
    /// False if syntax was incomplete or any require could not be resolved statically.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.modules.values().all(|module| {
            module.semantics.is_complete()
                && module
                    .dependencies
                    .iter()
                    .all(|dependency| matches!(dependency.resolution, Resolution::Resolved(_)))
        })
    }
}

/// Aliases come only from the selected Instar config; other formats remain separately retained.
/// Roblox mappings are read, never generated or inferred from project directories.
pub struct Resolver<'project> {
    project: &'project Project,
    aliases: BTreeMap<String, PathBuf>,
    roblox: Option<RobloxMap>,
}

impl<'project> Resolver<'project> {
    /// Load an explicitly configured sourcemap through the overlay-aware source store.
    /// Rojo relative file paths belong to its project directory, not the sourcemap directory.
    /// When no Rojo project is specified, the explicitly selected Instar root is the base.
    ///
    /// # Errors
    /// Returns configuration alias, source-acquisition or JSON failures with their owner.
    pub fn new(
        project: &'project Project,
        sources: &mut SourceStore,
    ) -> Result<Self, ResolveError> {
        let mut aliases = BTreeMap::new();
        if let Some(configured) = project.instar().and_then(|config| config.aliases.as_ref()) {
            for (name, path) in configured {
                if name.is_empty()
                    || matches!(name.as_str(), "." | "..")
                    || !name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
                    })
                    || aliases
                        .insert(
                            name.to_ascii_lowercase(),
                            normalize(&project.root().join(path)),
                        )
                        .is_some()
                {
                    return Err(ResolveError::Alias(name.clone()));
                }
            }
        }
        let mut roblox = None;
        if let Some(config) = project.instar().and_then(|config| config.roblox.as_ref())
            && let Some(path) = &config.sourcemap
        {
            let source = sources.read(&normalize(&project.root().join(path)))?;
            let root: MapNode = serde_json::from_slice(source.bytes()).map_err(|error| {
                ResolveError::Sourcemap {
                    path: source.path().to_owned(),
                    source: error,
                }
            })?;
            let base = config
                .project
                .as_ref()
                .map(|path| project.root().join(path));
            let base = base
                .as_deref()
                .and_then(Path::parent)
                .unwrap_or(project.root());
            let mut map = RobloxMap {
                source,
                nodes: Vec::new(),
            };
            map.insert(root, None, base);
            roblox = Some(map);
        }
        Ok(Self {
            project,
            aliases,
            roblox,
        })
    }

    /// Resolve one semantic require site belonging to these exact facts.
    ///
    /// # Errors
    /// Returns source/filesystem failures; ordinary missing, dynamic and ambiguous targets are outcomes.
    pub fn resolve(
        &self,
        sources: &mut SourceStore,
        facts: &Semantics,
        site: usize,
    ) -> Result<Resolution, ResolveError> {
        let Some(site) = facts.requires().get(site) else {
            return Ok(Resolution::Unsupported(
                "require site index is outside these facts",
            ));
        };
        let Some(argument) = &site.argument else {
            return Ok(Resolution::Dynamic);
        };
        let argument = unparenthesized(argument);
        if argument.kind() == K::LiteralExpression {
            return match literal(&argument) {
                Ok(path) => self.path(sources, facts.parse().source(), &path),
                Err(outcome) => Ok(outcome),
            };
        }
        if !instance_root(&argument, facts) {
            return Ok(Resolution::Dynamic);
        }
        let Some(map) = &self.roblox else {
            return Ok(Resolution::ContextRequired);
        };
        let instance = match map.expression(&argument, facts) {
            Ok(instance) => instance,
            Err(outcome) => return Ok(outcome),
        };
        let node = &map.nodes[instance];
        if node.class_name != "ModuleScript" {
            return Ok(Resolution::Unsupported(
                "required instance is not a ModuleScript",
            ));
        }
        let paths: Vec<_> = node
            .paths
            .iter()
            .filter(|path| is_luau(path))
            .cloned()
            .collect();
        match paths.as_slice() {
            [path] => Ok(match read_optional(sources, path)? {
                Some(source) => Resolution::Resolved(source),
                None => Resolution::Missing(paths),
            }),
            [] => Ok(Resolution::Missing(paths)),
            _ => Ok(Resolution::Ambiguous(paths)),
        }
    }

    /// Traverse explicitly supplied entry files, retaining unresolved edges and source revisions.
    /// Entry include/exclude selection is separate: dependencies are never filtered by entry globs.
    ///
    /// # Errors
    /// Returns source, UTF-8 or filesystem errors; cycles terminate through logical path identity.
    pub fn graph(
        &self,
        sources: &mut SourceStore,
        entries: &[PathBuf],
    ) -> Result<DependencyGraph, ResolveError> {
        let mut pending = VecDeque::new();
        let mut snapshots = BTreeMap::new();
        for path in entries {
            let path = normalize(&self.project.root().join(path));
            let source = sources.read(&path)?;
            if snapshots.insert(path, Arc::clone(&source)).is_none() {
                pending.push_back(source);
            }
        }
        let mut modules = BTreeMap::new();
        while let Some(source) = pending.pop_front() {
            let path = source.path().to_owned();
            let semantics = Semantics::new(Parse::new(source)?);
            let mut dependencies = Vec::new();
            for (index, site) in semantics.requires().iter().enumerate() {
                let mut resolution = self.resolve(sources, &semantics, index)?;
                if let Resolution::Resolved(target) = &mut resolution {
                    if let Some(retained) = snapshots.get(target.path()) {
                        *target = Arc::clone(retained);
                    } else {
                        snapshots.insert(target.path().to_owned(), Arc::clone(target));
                        pending.push_back(Arc::clone(target));
                    }
                }
                dependencies.push(Dependency {
                    range: site.range,
                    resolution,
                });
            }
            modules.insert(
                path,
                Module {
                    semantics,
                    dependencies,
                },
            );
        }
        Ok(DependencyGraph {
            configurations: self
                .project
                .files()
                .iter()
                .map(|file| ConfigFile {
                    path: file.path.clone(),
                    kind: file.kind,
                    bytes: file.bytes.clone(),
                })
                .collect(),
            sourcemap: self.roblox.as_ref().map(|map| Arc::clone(&map.source)),
            modules,
        })
    }

    fn path(
        &self,
        sources: &mut SourceStore,
        from: &Source,
        request: &str,
    ) -> Result<Resolution, ResolveError> {
        let request = request.replace('\\', "/");
        if request.contains('\0') {
            return Ok(Resolution::Unsupported("require path contains NUL"));
        }
        let base;
        let rest;
        if let Some(aliased) = request.strip_prefix('@') {
            let (name, tail) = aliased.split_once('/').unwrap_or((aliased, ""));
            let name = name.to_ascii_lowercase();
            if let Some(path) = self.aliases.get(&name) {
                // The pinned upstream default allows an explicit alias to override @self.
                base = path.clone();
                // A virtual descendant can make an alias useful before its directory is
                // on disk, but a concrete file/directory collision is still ambiguous.
                if let outcome @ Resolution::Ambiguous(_) = module_source(sources, &base)? {
                    return Ok(outcome);
                }
            } else if name == "self" {
                // Unlike ./, @self navigates from the requiring module itself. This is
                // what makes children of an init module addressable.
                base = module_path(from.path());
                if let Some(outcome) = navigation_failure(sources, &base)? {
                    return Ok(outcome);
                }
            } else {
                return Ok(Resolution::Unsupported(
                    "alias is not defined by the selected Instar configuration",
                ));
            }
            rest = tail;
        } else if request.starts_with("./") || request.starts_with("../") {
            // VfsNavigator::getModulePath treats init files as their containing module.
            let module = module_path(from.path());
            if let Some(outcome) = navigation_failure(sources, &module)? {
                return Ok(outcome);
            }
            base = module.parent().unwrap_or(&module).to_owned();
            rest = &request;
        } else {
            return Ok(Resolution::Unsupported(
                "require path must start with ./, ../ or @",
            ));
        }
        // A module component cannot replace the base with a Windows drive/root path.
        if rest.split('/').any(|part| part.contains(':')) {
            return Ok(Resolution::Unsupported(
                "require path contains a drive component",
            ));
        }
        let mut path = base;
        for part in rest
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
        {
            if part == ".." {
                if !path.pop() {
                    return Ok(Resolution::Missing(vec![path]));
                }
            } else {
                if part == ".config" {
                    return Ok(Resolution::Unsupported(".config is not a require module"));
                }
                path.push(part);
            }
            let failure = if part == ".." {
                parent_navigation_failure(sources, &path)?
            } else {
                navigation_failure(sources, &path)?
            };
            if let Some(outcome) = failure {
                return Ok(outcome);
            }
        }
        module_source(sources, &normalize(&path))
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut output = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                output.pop();
            }
            other => output.push(other),
        }
    }
    output
}

fn is_luau(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "luau" || extension == "lua")
}

fn module_path(path: &Path) -> PathBuf {
    if path
        .file_name()
        .is_some_and(|name| name == "init.luau" || name == "init.lua")
    {
        path.parent().unwrap_or(path).to_owned()
    } else if is_luau(path) {
        path.with_extension("")
    } else {
        path.to_owned()
    }
}

fn read_optional(
    sources: &mut SourceStore,
    path: &Path,
) -> Result<Option<Arc<Source>>, ResolveError> {
    match sources.read(path) {
        Ok(source) => Ok(Some(source)),
        Err(error @ SourceError::Io { .. }) => {
            if matches!(&error, SourceError::Io { source, .. } if source.kind() == io::ErrorKind::NotFound)
                && fs::symlink_metadata(path)
                    .is_err_and(|error| error.kind() == io::ErrorKind::NotFound)
            {
                Ok(None)
            } else {
                Err(error.into())
            }
        }
        Err(SourceError::NotFile(_)) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn navigation_failure(
    sources: &mut SourceStore,
    path: &Path,
) -> Result<Option<Resolution>, ResolveError> {
    match module_source(sources, path)? {
        Resolution::Resolved(_) => Ok(None),
        outcome @ Resolution::Ambiguous(_) => Ok(Some(outcome)),
        outcome @ Resolution::Missing(_) => match fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => Ok(None),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Some(outcome)),
            Err(source) => Err(SourceError::Io {
                path: path.to_owned(),
                source,
            }
            .into()),
            Ok(_) => Ok(Some(outcome)),
        },
        _ => unreachable!("module_source only returns file navigation outcomes"),
    }
}

fn parent_navigation_failure(
    sources: &mut SourceStore,
    path: &Path,
) -> Result<Option<Resolution>, ResolveError> {
    Ok(match navigation_failure(sources, path)? {
        Some(Resolution::Ambiguous(_)) => None,
        outcome => outcome,
    })
}

fn module_source(sources: &mut SourceStore, path: &Path) -> Result<Resolution, ResolveError> {
    let mut candidates = Vec::new();
    let mut files = Vec::new();
    // VfsNavigator rejects direct init paths and ambiguity, rather than preferring .luau.
    if path.file_name().is_none_or(|name| name != "init") {
        for suffix in [".luau", ".lua"] {
            let mut candidate = path.as_os_str().to_owned();
            candidate.push(suffix);
            let candidate = PathBuf::from(candidate);
            if let Some(source) = read_optional(sources, &candidate)? {
                files.push(source);
            }
            candidates.push(candidate);
        }
    }
    let directory = match fs::metadata(path) {
        Ok(metadata) => metadata.is_dir(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(source) => {
            return Err(SourceError::Io {
                path: path.to_owned(),
                source,
            }
            .into());
        }
    };
    let file_count = files.len();
    for name in ["init.luau", "init.lua"] {
        let candidate = path.join(name);
        if let Some(source) = read_optional(sources, &candidate)? {
            files.push(source);
        }
        candidates.push(candidate);
    }
    if files.len() > 1 || (directory && file_count != 0) {
        let mut paths: Vec<_> = files
            .iter()
            .map(|source| source.path().to_owned())
            .collect();
        if directory && file_count == files.len() {
            paths.push(path.to_owned());
        }
        return Ok(Resolution::Ambiguous(paths));
    }
    Ok(files
        .pop()
        .map_or(Resolution::Missing(candidates), Resolution::Resolved))
}

fn unparenthesized(node: &SyntaxNode) -> SyntaxNode {
    let mut node = node.clone();
    while matches!(
        node.kind(),
        K::ParenthesizedExpression | K::TypeAssertionExpression
    ) {
        let Some(inner) = node.children().next() else {
            break;
        };
        node = inner;
    }
    node
}

fn literal(node: &SyntaxNode) -> Result<String, Resolution> {
    let token = node
        .children_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|token| token.kind() == K::String)
        .ok_or(Resolution::Dynamic)?;
    let bytes =
        string_bytes(&token).map_err(|_| Resolution::Unsupported("malformed require string"))?;
    String::from_utf8(bytes).map_err(|_| Resolution::Unsupported("require string is not UTF-8"))
}

fn name(node: &SyntaxNode) -> Option<String> {
    node.descendants_with_tokens()
        .filter_map(rowan::NodeOrToken::into_token)
        .find(|token| token.kind() == K::Identifier)
        .map(|token| token.text().to_owned())
}

fn global(node: &SyntaxNode, facts: &Semantics) -> bool {
    facts.references().iter().any(|reference| {
        reference.namespace == Namespace::Value
            && reference.declaration.is_none()
            && facts
                .parse()
                .source_range(node.text_range())
                .contains_range(reference.range)
    })
}

fn instance_root(node: &SyntaxNode, facts: &Semantics) -> bool {
    let mut node = unparenthesized(node);
    loop {
        match node.kind() {
            K::NameExpression => {
                return global(&node, facts)
                    && name(&node).is_some_and(|name| {
                        matches!(name.as_str(), "script" | "game" | "workspace")
                    });
            }
            K::FieldExpression | K::IndexExpression | K::MethodExpression | K::CallExpression => {
                let Some(first) = node.children().next() else {
                    return false;
                };
                node = unparenthesized(&first);
            }
            _ => return false,
        }
    }
}

// Matches Rojo's emitted optional collections without assigning project configuration defaults.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MapNode {
    name: String,
    class_name: String,
    #[serde(default)]
    file_paths: Vec<PathBuf>,
    #[serde(default)]
    children: Vec<Self>,
}

struct Instance {
    name: String,
    class_name: String,
    paths: Vec<PathBuf>,
    parent: Option<usize>,
    children: Vec<usize>,
}

struct RobloxMap {
    source: Arc<Source>,
    nodes: Vec<Instance>,
}

impl RobloxMap {
    fn insert(&mut self, node: MapNode, parent: Option<usize>, base: &Path) -> usize {
        let id = self.nodes.len();
        self.nodes.push(Instance {
            name: node.name,
            class_name: node.class_name,
            paths: node
                .file_paths
                .into_iter()
                .map(|path| normalize(&base.join(path)))
                .collect(),
            parent,
            children: Vec::new(),
        });
        for child in node.children {
            let child = self.insert(child, Some(id), base);
            self.nodes[id].children.push(child);
        }
        id
    }

    fn unique(&self, nodes: &[usize]) -> Result<usize, Resolution> {
        match nodes {
            [id] => Ok(*id),
            [] => Err(Resolution::Missing(Vec::new())),
            _ => Err(Resolution::Ambiguous(
                nodes
                    .iter()
                    .flat_map(|id| self.nodes[*id].paths.clone())
                    .collect(),
            )),
        }
    }

    fn child(&self, parent: usize, child: &str, service: bool) -> Result<usize, Resolution> {
        let ids: Vec<_> = self.nodes[parent]
            .children
            .iter()
            .copied()
            .filter(|id| {
                if service {
                    self.nodes[*id].class_name == child
                } else {
                    self.nodes[*id].name == child
                }
            })
            .collect();
        self.unique(&ids)
    }

    fn expression(&self, node: &SyntaxNode, facts: &Semantics) -> Result<usize, Resolution> {
        let node = unparenthesized(node);
        match node.kind() {
            K::NameExpression if global(&node, facts) => match name(&node).as_deref() {
                Some("script") => {
                    let path = normalize(facts.parse().source().path());
                    let ids: Vec<_> = self
                        .nodes
                        .iter()
                        .enumerate()
                        .filter(|(_, node)| node.paths.contains(&path))
                        .map(|(id, _)| id)
                        .collect();
                    self.unique(&ids)
                }
                Some("game") if self.nodes[0].class_name == "DataModel" => Ok(0),
                Some("workspace") if self.nodes[0].class_name == "DataModel" => {
                    self.child(0, "Workspace", true)
                }
                _ => Err(Resolution::ContextRequired),
            },
            K::FieldExpression | K::IndexExpression => {
                let mut parts = node.children();
                let parent = self.expression(&parts.next().ok_or(Resolution::Dynamic)?, facts)?;
                let field = parts.next().ok_or(Resolution::Dynamic)?;
                let field = if node.kind() == K::FieldExpression {
                    name(&field).ok_or(Resolution::Dynamic)?
                } else {
                    literal(&unparenthesized(&field))?
                };
                if field == "Parent" {
                    self.nodes[parent]
                        .parent
                        .ok_or(Resolution::Missing(Vec::new()))
                } else {
                    self.child(parent, &field, false)
                }
            }
            K::CallExpression => {
                let mut parts = node.children();
                let method = parts
                    .next()
                    .filter(|node| node.kind() == K::MethodExpression)
                    .ok_or(Resolution::Dynamic)?;
                let mut callee = method.children();
                let parent = self.expression(&callee.next().ok_or(Resolution::Dynamic)?, facts)?;
                let method =
                    name(&callee.next().ok_or(Resolution::Dynamic)?).ok_or(Resolution::Dynamic)?;
                let args = parts.next().ok_or(Resolution::Dynamic)?;
                let args = args.children().next().ok_or(Resolution::Dynamic)?;
                let mut args = args.children();
                let argument = literal(&unparenthesized(&args.next().ok_or(Resolution::Dynamic)?))?;
                if args.next().is_some() {
                    return Err(Resolution::Dynamic);
                }
                match method.as_str() {
                    "GetService" if parent == 0 && self.nodes[0].class_name == "DataModel" => {
                        self.child(parent, &argument, true)
                    }
                    "WaitForChild" | "FindFirstChild" => self.child(parent, &argument, false),
                    _ => Err(Resolution::Dynamic),
                }
            }
            _ => Err(Resolution::Dynamic),
        }
    }
}
