use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    native,
    project::InstarConfig,
    source::{Source, SourceStore, absolute},
};

#[derive(Clone)]
pub(crate) struct Configuration {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    aliases: BTreeMap<String, String>,
}

struct Node {
    is_directory: bool,
    module: Option<PathBuf>,
}

#[derive(Clone)]
struct Alias {
    configuration: PathBuf,
    value: String,
}

/// One operation's module identities, source snapshots and configuration snapshots.
pub struct Resolver<'store> {
    sources: &'store mut SourceStore,
    snapshots: BTreeMap<PathBuf, Arc<Source>>,
    configurations: BTreeMap<PathBuf, Vec<Configuration>>,
    aliases: BTreeMap<PathBuf, BTreeMap<String, Alias>>,
    contents: BTreeMap<PathBuf, Option<Vec<u8>>>,
}

impl<'store> Resolver<'store> {
    pub fn new(sources: &'store mut SourceStore) -> Self {
        Self {
            sources,
            snapshots: BTreeMap::new(),
            configurations: BTreeMap::new(),
            aliases: BTreeMap::new(),
            contents: BTreeMap::new(),
        }
    }

    /// Load a module once, preferring editor-owned bytes to disk.
    ///
    /// # Errors
    /// Returns path and source-loading failures.
    pub fn load(&mut self, path: &Path) -> io::Result<Arc<Source>> {
        let path = absolute(path).map_err(io::Error::other)?;
        if let Some(source) = self.snapshots.get(&path) {
            return Ok(Arc::clone(source));
        }
        let source = self.sources.read(&path).map_err(io::Error::other)?;
        self.snapshots.insert(path, Arc::clone(&source));
        Ok(source)
    }

    fn is_file(&self, path: &Path) -> io::Result<bool> {
        let identity = absolute(path).map_err(io::Error::other)?;
        if self.snapshots.contains_key(&identity)
            || self.sources.is_open(path).map_err(io::Error::other)?
        {
            return Ok(true);
        }
        Ok(metadata(path)?.is_some_and(|metadata| metadata.is_file()))
    }

    fn contents(&mut self, path: &Path) -> io::Result<Option<&[u8]>> {
        if !self.contents.contains_key(path) {
            let exists = match fs::symlink_metadata(path) {
                Ok(_) => true,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                    ) =>
                {
                    false
                }
                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ));
                }
            };
            let contents = if exists {
                Some(fs::read(path).map_err(|error| {
                    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
                })?)
            } else {
                None
            };
            self.contents.insert(path.to_owned(), contents);
        }
        Ok(self.contents[path].as_deref())
    }

    pub(crate) fn configurations(&mut self, from: &Path) -> io::Result<&[Configuration]> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;
        self.project_aliases(directory)?;
        if !self.configurations.contains_key(directory) {
            let mut configurations = Vec::new();
            for ancestor in directory.ancestors().collect::<Vec<_>>().into_iter().rev() {
                let executable = ancestor.join(".config.luau");
                if self.contents(&executable)?.is_some() {
                    return Err(io::Error::other(format!(
                        "{}: executable configuration is not supported by the native integration",
                        executable.display()
                    )));
                }
                let path = ancestor.join(".luaurc");
                let Some(bytes) = self.contents(&path)? else {
                    continue;
                };
                let bytes = bytes.to_vec();
                let aliases = native::aliases(&bytes)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
                configurations.push(Configuration {
                    path,
                    bytes,
                    aliases,
                });
            }
            self.configurations
                .insert(directory.to_owned(), configurations);
        }
        Ok(&self.configurations[directory])
    }

    fn project_aliases(&mut self, directory: &Path) -> io::Result<&BTreeMap<String, Alias>> {
        if !self.aliases.contains_key(directory) {
            let mut aliases = BTreeMap::new();
            for ancestor in directory.ancestors() {
                let path = ancestor.join("instar.toml");
                let Some(contents) = self.contents(&path)? else {
                    continue;
                };
                let contents = std::str::from_utf8(contents)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
                let configuration = InstarConfig::parse(contents)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
                let configured = configuration.aliases.unwrap_or_default();
                let validation = serde_json::to_vec(&serde_json::json!({"aliases": &configured}))?;
                native::aliases(&validation)
                    .map_err(|error| io::Error::other(format!("{}: {error}", path.display())))?;
                for (alias, target) in configured {
                    let key = alias.to_ascii_lowercase();
                    let value = target
                        .to_str()
                        .ok_or_else(|| io::Error::other("alias path requires UTF-8"))?
                        .to_owned();
                    if aliases
                        .insert(
                            key,
                            Alias {
                                configuration: path.clone(),
                                value,
                            },
                        )
                        .is_some()
                    {
                        return Err(io::Error::other(format!(
                            "{}: duplicate case-insensitive alias {alias}",
                            path.display()
                        )));
                    }
                }
                break;
            }
            self.aliases.insert(directory.to_owned(), aliases);
        }
        Ok(&self.aliases[directory])
    }

    fn alias(&mut self, from: &Path, name: &str) -> io::Result<Option<Alias>> {
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;
        if let Some(alias) = self.project_aliases(directory)?.get(name) {
            return Ok(Some(alias.clone()));
        }
        Ok(self
            .configurations(from)?
            .iter()
            .rev()
            .find_map(|configuration| {
                Some(Alias {
                    configuration: configuration.path.clone(),
                    value: configuration.aliases.get(name)?.clone(),
                })
            }))
    }

    /// Resolve a static require specifier to a module identity without rewriting it.
    ///
    /// # Errors
    /// Returns filesystem, configuration and ambiguous-module failures.
    pub fn resolve(&mut self, from: &Path, specifier: &str) -> io::Result<Option<PathBuf>> {
        let from = absolute(from).map_err(io::Error::other)?;
        self.configurations(&from)?;
        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;
        let base = if matches!(
            from.file_name().and_then(|name| name.to_str()),
            Some("init.lua" | "init.luau")
        ) {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };
        let specifier = specifier.replace('\\', "/");
        let (origin, target) = if specifier.starts_with("./") || specifier.starts_with("../") {
            (base.to_owned(), base.join(&specifier))
        } else if specifier.starts_with('@') {
            let mut context = from.clone();
            let mut value = specifier.clone();
            let mut tails = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            while let Some(rest) = value.strip_prefix('@') {
                let (name, tail) = rest.split_once('/').unwrap_or((rest, ""));
                tails.push(tail.trim_start_matches('/').to_owned());
                let name = name.to_ascii_lowercase();
                if name == "self" {
                    // Pinned Luau's default makes @self refer to the requiring
                    // module node, regardless of configured aliases named self.
                    let node = if from.file_stem().is_some_and(|stem| stem == "init") {
                        directory.to_owned()
                    } else {
                        from.with_extension("")
                    };
                    context.clone_from(&from);
                    node.to_str()
                        .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?
                        .clone_into(&mut value);
                    break;
                }
                let Some(alias) = self.alias(&context, &name)? else {
                    return Ok(None);
                };
                if !seen.insert((alias.configuration.clone(), name)) {
                    return Err(io::Error::other(format!(
                        "{}: cyclic alias in {specifier}",
                        alias.configuration.display()
                    )));
                }
                context = alias.configuration;
                value = alias.value.replace('\\', "/");
            }
            let origin = context
                .parent()
                .ok_or_else(|| io::Error::other("alias configuration has no parent"))?
                .to_owned();
            let mut target = origin.join(value);
            for tail in tails.into_iter().rev() {
                if !tail.is_empty() {
                    // A tail is a sequence of child names, never a new root.
                    target.as_mut_os_string().push("/");
                    target.as_mut_os_string().push(tail);
                }
            }
            (origin, target)
        } else {
            return Ok(None);
        };
        self.walk(&origin, &target)
    }

    fn walk(&self, origin: &Path, target: &Path) -> io::Result<Option<PathBuf>> {
        use std::path::Component;
        let (mut current, remaining) = match target.strip_prefix(origin) {
            Ok(remaining) => (origin.to_owned(), remaining),
            Err(_) => (PathBuf::new(), target),
        };
        let mut node = if current.as_os_str().is_empty() {
            Node {
                is_directory: true,
                module: None,
            }
        } else {
            let Some(node) = self.node(&current)? else {
                return Ok(None);
            };
            node
        };
        for component in remaining.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    current.push(component);
                    continue;
                }
                Component::CurDir => continue,
                Component::ParentDir => {
                    if !current.pop() {
                        return Ok(None);
                    }
                }
                Component::Normal(_) => {
                    if !node.is_directory {
                        return Ok(None);
                    }
                    current
                        .as_mut_os_string()
                        .push(std::path::MAIN_SEPARATOR_STR);
                    current.as_mut_os_string().push(component.as_os_str());
                }
            }
            let Some(next) = self.node(&current)? else {
                return Ok(None);
            };
            node = next;
        }
        node.module
            .map(|module| absolute(&module).map_err(io::Error::other))
            .transpose()
    }

    fn node(&self, target: &Path) -> io::Result<Option<Node>> {
        let mut candidates = Vec::new();
        // Match pinned Luau VfsNavigator: suffixes append to dotted names;
        // a file and a directory with the same module name are ambiguous.
        if target.file_name().is_some_and(|name| name != "init") {
            for suffix in [".luau", ".lua"] {
                let mut path = target.as_os_str().to_owned();
                path.push(suffix);
                let path = PathBuf::from(path);
                if self.is_file(&path)? {
                    candidates.push(path);
                }
            }
        }
        let mut initializers = Vec::new();
        for filename in ["init.luau", "init.lua"] {
            let path = target.join(filename);
            if self.is_file(&path)? {
                initializers.push(path);
            }
        }
        let directory = !initializers.is_empty()
            || self
                .sources
                .has_open_descendants(target)
                .map_err(io::Error::other)?
            || metadata(target)?.is_some_and(|metadata| metadata.is_dir());
        if candidates.len() > 1 || initializers.len() > 1 || (!candidates.is_empty() && directory) {
            return Err(io::Error::other(format!(
                "{}: ambiguous module",
                target.display()
            )));
        }
        let module = candidates.pop().or_else(|| initializers.pop());
        Ok((directory || module.is_some()).then_some(Node {
            is_directory: directory,
            module,
        }))
    }
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}
