//! Tracked filesystem and sourcemap module resolution.

use std::{
    collections::BTreeSet,
    io,
    path::{Path, PathBuf},
    rc::Rc,
};

use instar_analysis::error::invalid;

use crate::{
    project::Project,
    source::{Failure, Kind, normalize},
};

/// A module identity independent of its backing source.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Identity {
    /// Abstract extensionless filesystem navigation path.
    Filesystem(PathBuf),

    /// One placement in one immutable sourcemap context.
    Instance {
        /// Absolute map path.
        map: PathBuf,
        /// Map source revision.
        revision: u64,
        /// Instance index within that revision.
        node: usize,
    },
}

/// An abstract module and its backing source file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Module {
    /// Graph and native analysis identity.
    pub identity: Identity,

    /// Absolute navigation path without extension or trailing init filename.
    pub path: PathBuf,

    /// Absolute backing source, preserving symlink spelling.
    pub source: PathBuf,
}

/// Starting instance for a static Roblox request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Root {
    /// The caller's exact instance context.
    Script,

    /// The `DataModel` root of the caller's map.
    Game,

    /// The Workspace service of the caller's map.
    Workspace,
}

/// A statically known instance navigation operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Navigate to the mapped parent.
    Parent,

    /// Navigate to a uniquely named child or descendant.
    Child {
        /// Exact instance name.
        name: String,
        /// Search recursively.
        recursive: bool,
    },

    /// Select a service by its class name.
    Service(String),
}

/// A statically known require request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// RFC 48 filesystem navigation.
    String(String),

    /// Sourcemap-backed instance navigation.
    Instance {
        /// Starting context.
        root: Root,
        /// Ordered navigation operations.
        steps: Vec<Step>,
    },
}

/// A retained resolution, including all consulted and missing candidates.
#[derive(Clone, Debug)]
pub struct Resolution {
    /// Resolved module or retained error.
    pub result: Result<Module, Failure>,

    /// Files, directories, settings and maps consulted by this resolution.
    pub inputs: BTreeSet<PathBuf>,
}

impl Project {
    /// Enumerates explicit instance contexts without choosing an ambiguous placement.
    ///
    /// # Errors
    /// Returns source, settings or sourcemap errors.
    pub fn contexts(&mut self, source: &Path) -> io::Result<Vec<Module>> {
        validate_source(source)?;
        let source = normalize(source);

        let directory = source
            .parent()
            .ok_or_else(|| invalid("source has no parent"))?;

        let mut modules = Vec::new();

        for map in self.maps(directory)? {
            for identity in map.placements(&source) {
                modules.push(map.module(map.index(&identity)?, false)?);
            }
        }

        if modules.is_empty() {
            modules.push(self.module(&source, None)?);
        }

        self.source(&source)?;

        Ok(modules)
    }

    /// Identifies an absolute entry source, rejecting ambiguous placements.
    ///
    /// # Errors
    /// Returns invalid source, configuration, map or ambiguous placement errors.
    pub fn module(&mut self, source: &Path, context: Option<&Identity>) -> io::Result<Module> {
        validate_source(source)?;
        let source = normalize(source);

        let directory = source
            .parent()
            .ok_or_else(|| invalid("source has no parent"))?;

        if let Some(context @ Identity::Instance { map: location, .. }) = context {
            self.configuration(directory)?;
            let map = self.map(location)?;
            let module = map.module(map.index(context)?, false)?;

            if module.source != source {
                return Err(invalid("source is not in the selected instance context"));
            }

            self.source(&module.source)?;

            return Ok(module);
        }

        let maps = self.maps(directory)?;
        let mut placements = Vec::new();

        for map in &maps {
            placements.extend(map.placements(&source));
        }

        if context.is_some() && !placements.is_empty() {
            return Err(invalid(
                "source has an instance context, not a filesystem context",
            ));
        }

        match placements.as_slice() {
            [identity] => {
                let map = maps
                    .iter()
                    .find(|map| map.index(identity).is_ok())
                    .ok_or_else(|| invalid("sourcemap context is unavailable"))?;

                let module = map.module(map.index(identity)?, false)?;
                self.source(&module.source)?;

                Ok(module)
            }

            [] => {
                let path = module_path(&source);

                if self.probe(&path)?.as_ref() != Some(&source) {
                    return Err(invalid(format!(
                        "source does not represent module {}",
                        path.display()
                    )));
                }

                let identity = Identity::Filesystem(path.clone());

                if context.is_some_and(|context| context != &identity) {
                    return Err(invalid(
                        "source does not match the selected filesystem context",
                    ));
                }

                Ok(Module {
                    identity,
                    path,
                    source,
                })
            }

            _ => Err(invalid(format!(
                "source {} maps to multiple instances",
                source.display()
            ))),
        }
    }

    /// Resolves a string from an entry source through the same tracked policy as graph sites.
    #[must_use]
    pub fn resolve_source(&mut self, source: &Path, request: &str) -> Resolution {
        let outer = std::mem::take(&mut self.view.consulted);

        let result = (|| {
            validate_source(source)?;
            validate_request(request)?;
            let module = self.module(source, None)?;

            self.resolve_inner(&module, &Request::String(request.to_owned()))
        })()
        .map_err(|error| {
            Failure::from(io::Error::new(
                error.kind(),
                format!("{}: require {request:?}: {error}", source.display()),
            ))
        });

        let inputs = std::mem::take(&mut self.view.consulted);
        self.view.consulted = outer;
        self.view.consulted.extend(inputs.iter().cloned());

        Resolution { result, inputs }
    }

    /// Resolves a static graph request and retains every consulted candidate on failure.
    #[must_use]
    pub fn resolve(&mut self, module: &Module, request: &Request) -> Resolution {
        let outer = std::mem::take(&mut self.view.consulted);
        let result = self.resolve_inner(module, request).map_err(Failure::from);
        let inputs = std::mem::take(&mut self.view.consulted);
        self.view.consulted = outer;
        self.view.consulted.extend(inputs.iter().cloned());

        Resolution { result, inputs }
    }

    fn resolve_inner(&mut self, module: &Module, request: &Request) -> io::Result<Module> {
        let current = self.module(&module.source, Some(&module.identity))?;

        match request {
            Request::String(request) => {
                validate_request(request)?;
                let request = request.replace('\\', "/");

                let directory = current
                    .source
                    .parent()
                    .ok_or_else(|| invalid("source has no parent"))?;

                let settings = self.configuration(directory)?;

                let target = if let Some(aliased) = request.strip_prefix('@') {
                    let (name, rest) = aliased.split_once('/').unwrap_or((aliased, ""));

                    let start =
                        self.alias(&settings.snapshot, name, &current.path, &mut Vec::new())?;

                    self.walk(start, rest)?
                } else {
                    let parent = current
                        .path
                        .parent()
                        .ok_or_else(|| invalid("module has no parent"))?;

                    self.walk(parent.to_path_buf(), &request)?
                };

                let source = self.probe(&target)?.ok_or_else(|| {
                    invalid(format!(
                        "directory has no module source: {}",
                        target.display()
                    ))
                })?;

                self.target(&current, &source)
            }

            Request::Instance { root, steps } => {
                let directory = current
                    .source
                    .parent()
                    .ok_or_else(|| invalid("source has no parent"))?;

                let map = if let Identity::Instance { map: location, .. } = &current.identity {
                    self.map(location)?
                } else {
                    let maps = self.maps(directory)?;

                    match maps.as_slice() {
                        [map] => Rc::clone(map),
                        [] => return Err(invalid("no sourcemap configured")),

                        _ => {
                            return Err(invalid(
                                "instance request has multiple sourcemap contexts",
                            ));
                        }
                    }
                };

                let mut node = match root {
                    Root::Script => map.index(&current.identity)?,
                    Root::Game => map.root()?,
                    Root::Workspace => map.service(map.root()?, "Workspace")?,
                };

                for step in steps {
                    node = match step {
                        Step::Parent => map.parent(node)?,
                        Step::Child { name, recursive } => map.child(node, name, *recursive)?,
                        Step::Service(name) => map.service(node, name)?,
                    };
                }

                let target = map.module(node, true)?;
                self.source(&target.source)?;

                Ok(target)
            }
        }
    }

    fn target(&mut self, current: &Module, source: &Path) -> io::Result<Module> {
        if let Identity::Instance { map: location, .. } = &current.identity {
            let map = self.map(location)?;
            let placements = map.placements(source);

            match placements.as_slice() {
                [identity] => {
                    let module = map.module(map.index(identity)?, true)?;
                    self.source(&module.source)?;

                    return Ok(module);
                }

                [] => {}

                _ => {
                    return Err(invalid(format!(
                        "source {} maps to multiple instances in the caller's context",
                        source.display()
                    )));
                }
            }
        }

        let module = self.module(source, None)?;

        if let Identity::Instance { map: location, .. } = &module.identity {
            let map = self.map(location)?;

            map.module(map.index(&module.identity)?, true)
        } else {
            Ok(module)
        }
    }

    fn alias(
        &mut self,
        snapshot: &instar_bridge::Snapshot,
        name: &str,
        module: &Path,
        stack: &mut Vec<(PathBuf, String)>,
    ) -> io::Result<PathBuf> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || b".-_".contains(&character))
        {
            return Err(invalid(format!("invalid alias name @{name}")));
        }

        let name = name.to_ascii_lowercase();

        if name == "self" {
            return Ok(module.to_path_buf());
        }

        let definition = snapshot
            .aliases
            .get(&name)
            .ok_or_else(|| invalid(format!("unknown alias @{name}")))?;

        let key = (definition.directory.clone(), name.clone());

        if stack.contains(&key) {
            let mut cycle = stack
                .iter()
                .map(|(directory, name)| format!("{}:@{name}", directory.display()))
                .collect::<Vec<_>>();

            cycle.push(format!("{}:@{name}", definition.directory.display()));

            return Err(invalid(format!("alias cycle: {}", cycle.join(" -> "))));
        }

        stack.push(key);
        let value = definition.value.replace('\\', "/");

        if value.contains('\0') || value.is_empty() {
            return Err(invalid(format!(
                "alias @{name} target must be nonempty and contain no NUL"
            )));
        }

        let result = if let Some(aliased) = value.strip_prefix('@') {
            let (next, rest) = aliased.split_once('/').unwrap_or((aliased, ""));
            let settings = self.configuration(&definition.directory)?;
            let start = self.alias(&settings.snapshot, next, module, stack)?;

            self.walk(start, rest)
        } else if Path::new(&value).is_absolute() {
            let target = module_path(&normalize(Path::new(&value)));
            self.probe(&target)?;

            Ok(target)
        } else {
            self.walk(definition.directory.clone(), &value)
        };

        stack.pop();

        result
    }

    fn walk(&mut self, mut path: PathBuf, request: &str) -> io::Result<PathBuf> {
        for component in request.split('/') {
            match component {
                "" | "." => {}

                ".." => {
                    if !path.pop() {
                        return Err(invalid("module navigation escaped the filesystem root"));
                    }
                }

                ".config" | ".config.luau" => {
                    return Err(invalid(".config.luau is not an importable module"));
                }

                name => {
                    if name.contains(':') {
                        return Err(invalid("invalid module path component"));
                    }

                    path.push(name);
                    self.probe(&path)?;
                }
            }
        }

        Ok(path)
    }

    pub(crate) fn probe(&mut self, path: &Path) -> io::Result<Option<PathBuf>> {
        if path.file_name().is_some_and(|name| name == ".config") {
            return Err(invalid(".config.luau is not an importable module"));
        }

        let directory = self.view.kind(path)? == Some(Kind::Directory);
        let mut sources = Vec::new();

        if path.file_name().is_some_and(|name| name != "init") {
            for suffix in [".luau", ".lua"] {
                let mut filename = path.as_os_str().to_os_string();
                filename.push(suffix);
                let candidate = PathBuf::from(filename);

                if self.view.kind(&candidate)? == Some(Kind::File) {
                    sources.push(candidate);
                }
            }
        }

        let sibling = !sources.is_empty();

        if directory {
            if sibling {
                sources.push(path.to_path_buf());
            } else {
                for filename in ["init.luau", "init.lua"] {
                    let candidate = path.join(filename);

                    if self.view.kind(&candidate)? == Some(Kind::File) {
                        sources.push(candidate);
                    }
                }
            }
        }

        if sources.len() > 1 {
            return Err(invalid(format!(
                "ambiguous module {}: {}",
                path.display(),
                sources
                    .iter()
                    .map(|source| source.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            )));
        }

        if let Some(source) = sources.pop() {
            return Ok(Some(source));
        }

        if directory {
            return Ok(None);
        }

        Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("module path not found: {}", path.display()),
        ))
    }
}

fn validate_source(source: &Path) -> io::Result<()> {
    if !source.is_absolute() {
        return Err(invalid("requiring source must be absolute"));
    }

    if source
        .file_name()
        .is_some_and(|name| name == ".config.luau")
    {
        return Err(invalid(".config.luau is not an importable module"));
    }

    if !matches!(
        source.extension().and_then(|name| name.to_str()),
        Some("lua" | "luau")
    ) {
        return Err(invalid(
            "requiring source must have a .lua or .luau extension",
        ));
    }

    Ok(())
}

fn validate_request(request: &str) -> io::Result<()> {
    if request.contains('\0') {
        return Err(invalid("require path contains NUL"));
    }

    let request = request.replace('\\', "/");

    if !request.starts_with('@') && !request.starts_with("./") && !request.starts_with("../") {
        return Err(invalid("require path must start with ./, ../, or @"));
    }

    Ok(())
}

pub(crate) fn module_path(source: &Path) -> PathBuf {
    if matches!(
        source.file_name().and_then(|name| name.to_str()),
        Some("init.lua" | "init.luau")
    ) {
        source.parent().unwrap_or(source).to_path_buf()
    } else if matches!(
        source.extension().and_then(|name| name.to_str()),
        Some("lua" | "luau")
    ) {
        source.with_extension("")
    } else {
        source.to_path_buf()
    }
}
