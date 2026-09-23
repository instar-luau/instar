//! Cached filesystem and sourcemap-backed require resolution.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{absolute, project::Project, roblox::Instance};

/// A module's navigation identity and its separate backing source file.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct Module {
    /// Filesystem module path; mapped modules navigate through their instance instead.
    pub path: PathBuf,

    /// Absolute source path, preserving symlink spelling.
    pub source: PathBuf,

    /// Sourcemap identity, distinct from the backing file.
    pub instance: Option<Instance>,
}

/// An observable reason a require could not resolve.
#[derive(Clone, Debug, Serialize, thiserror::Error)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum Failure {
    /// Unsupported require syntax or path.
    #[error("{0}")]
    Invalid(String),

    /// The require argument cannot be determined statically.
    #[error("{0}")]
    Dynamic(String),

    /// A sourcemap navigation, mapping, or class constraint failed.
    #[error("Roblox: {0}")]
    Roblox(String),

    /// A navigation component did not exist.
    #[error("module path not found: {0:?}")]
    Missing(PathBuf),

    /// More than one filesystem representation matched a module.
    #[error("ambiguous module {path:?}: {candidates:?}")]
    Ambiguous {
        /// Conflicting abstract module path.
        path: PathBuf,
        /// Matching files or directory.
        candidates: Vec<PathBuf>,
    },

    /// Navigation ended at a directory with no importable source.
    #[error("directory has no module source: {0:?}")]
    Directory(PathBuf),

    /// No alias definition was visible from the lookup scope.
    #[error("unknown alias @{0}")]
    UnknownAlias(String),

    /// Alias definitions formed a cycle.
    #[error("alias cycle: {0:?}")]
    AliasCycle(Vec<String>),

    /// Configuration could not be loaded or validated.
    #[error("configuration: {0}")]
    Configuration(String),

    /// A filesystem operation failed for a reason other than absence.
    #[error("filesystem: {0}")]
    Io(String),
}

/// A require outcome with the inputs consulted to obtain it.
#[derive(Clone, Debug, Serialize)]
pub struct Resolution {
    /// Resolved module or the concrete failure.
    pub result: Result<Module, Failure>,

    /// Filesystem probes, including absent candidates that could introduce ambiguity.
    pub candidates: BTreeSet<PathBuf>,

    /// Configuration probes, including absent configuration files.
    pub configurations: BTreeSet<PathBuf>,

    /// Sourcemaps consulted to resolve this request.
    pub sourcemaps: BTreeSet<PathBuf>,
}

#[derive(Clone)]
struct Entry {
    source: Option<PathBuf>,
}

#[derive(Clone)]
struct Lookup {
    result: Result<Entry, Failure>,
    candidates: Vec<PathBuf>,
}

enum Navigation {
    File(PathBuf),
    Instance(Instance),
}

/// Filesystem navigation cache for one immutable project snapshot.
#[derive(Default)]
pub struct Resolver {
    entries: HashMap<PathBuf, Lookup>,
    resolutions: HashMap<(Module, String), Resolution>,
}

impl Resolver {
    /// Creates an empty navigation cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns an import argument that resolves to the target in every source context.
    ///
    /// # Errors
    /// Returns invalid entry or configuration errors.
    pub fn import_argument(
        &mut self,
        project: &mut Project,
        source: &Path,
        target: &Path,
    ) -> Result<Option<String>, Failure> {
        let origins = self.entries(project, source)?;
        let mut targets = self.entries(project, target)?;

        for map in project
            .sourcemaps_for(source)
            .map_err(|error| Failure::Configuration(error.to_string()))?
        {
            for instance in map.instances_for_source(target) {
                if instance.class_name() == "ModuleScript" {
                    targets.push(instance.module(true)?);
                }
            }
        }

        let config = project
            .configuration(source)
            .map_err(|error| Failure::Configuration(error.to_string()))?;

        let mut candidates = BTreeSet::new();

        for origin in &origins {
            let mut roots: Vec<_> = config
                .aliases
                .keys()
                .map(|name| format!("@{name}"))
                .collect();

            roots.extend(["@game".to_owned(), "@self".to_owned(), "./".to_owned()]);

            for prefix in roots {
                let mut trace = Resolution {
                    result: Err(Failure::Invalid(String::new())),
                    candidates: BTreeSet::new(),
                    configurations: BTreeSet::new(),
                    sourcemaps: BTreeSet::new(),
                };

                let Ok(base) = self.request_navigation(project, origin, &prefix, &mut trace) else {
                    continue;
                };

                for destination in &targets {
                    let relative = match (&base, &destination.instance) {
                        (Navigation::File(base), None) => relative_path(base, &destination.path),

                        (Navigation::Instance(base), Some(instance)) => {
                            relative_instance(base, instance)
                        }

                        _ => None,
                    };

                    if let Some(relative) = relative {
                        let request = if prefix == "./" {
                            if relative.starts_with("../") {
                                relative
                            } else {
                                format!("./{relative}")
                            }
                        } else if relative == "." {
                            prefix.clone()
                        } else if !relative.starts_with("..") {
                            format!("{prefix}/{relative}")
                        } else {
                            continue;
                        };

                        candidates.insert(request);
                    }
                }
            }
        }

        let mut candidates: Vec<_> = candidates.into_iter().collect();

        candidates
            .sort_by_key(|request| (!request.starts_with('@'), request.len(), request.clone()));

        for request in candidates {
            if origins.iter().all(|origin| {
                self.resolve(project, origin, &request)
                    .result
                    .is_ok_and(|module| module.source == target)
            }) {
                if let Some(tail) = request.strip_prefix("@game/") {
                    let mut expression = "game".to_owned();

                    for component in tail.split('/') {
                        expression.push('[');

                        expression.push_str(
                            &serde_json::to_string(component)
                                .map_err(|error| Failure::Invalid(error.to_string()))?,
                        );

                        expression.push(']');
                    }

                    return Ok(Some(expression));
                }

                return serde_json::to_string(&request)
                    .map(Some)
                    .map_err(|error| Failure::Invalid(error.to_string()));
            }
        }

        Ok(None)
    }

    /// Maps an entry file to every instance context, or to one filesystem module if unmapped.
    ///
    /// # Errors
    /// Rejects missing, unsupported, ambiguous, or invalidly configured entry files.
    pub fn entries(
        &mut self,
        project: &mut Project,
        source: &Path,
    ) -> Result<Vec<Module>, Failure> {
        let source = absolute(source).map_err(|e| Failure::Io(e.to_string()))?;

        if !matches!(
            source.extension().and_then(|s| s.to_str()),
            Some("lua" | "luau")
        ) {
            return Err(Failure::Invalid(format!(
                "expected a .lua or .luau source: {}",
                source.display()
            )));
        }

        let mut modules = Vec::new();

        for map in project
            .sourcemaps_for(&source)
            .map_err(|e| Failure::Configuration(e.to_string()))?
        {
            for instance in map.instances_for_source(&source) {
                modules.push(instance.module(false)?);
            }
        }

        if !modules.is_empty() {
            return Ok(modules);
        }

        let path = module_path(&source);
        let lookup = self.lookup(project, &path);
        let entry = lookup.result?;

        match entry.source {
            Some(found) if found == source => Ok(vec![Module {
                path,
                source,
                instance: None,
            }]),

            _ => Err(Failure::Missing(source)),
        }
    }

    /// Resolves a string require from an abstract module, caching success and failure.
    pub fn resolve(&mut self, project: &mut Project, from: &Module, request: &str) -> Resolution {
        let key = (from.clone(), request.to_owned());

        if let Some(result) = self.resolutions.get(&key) {
            return result.clone();
        }

        let mut trace = Resolution {
            result: Err(Failure::Invalid(String::new())),
            candidates: BTreeSet::new(),
            configurations: BTreeSet::new(),
            sourcemaps: BTreeSet::new(),
        };

        trace.result = self.resolve_inner(project, from, request, &mut trace);
        self.resolutions.insert(key, trace.clone());

        trace
    }

    pub(crate) fn resolve_instance(instance: &Instance) -> Resolution {
        let result = instance.module(true);

        let candidates = result
            .as_ref()
            .ok()
            .map(|module| module.source.clone())
            .into_iter()
            .collect();

        Resolution {
            result,
            candidates,
            configurations: BTreeSet::new(),
            sourcemaps: [instance.map.path.clone()].into(),
        }
    }

    fn resolve_inner(
        &mut self,
        project: &mut Project,
        from: &Module,
        request: &str,
        trace: &mut Resolution,
    ) -> Result<Module, Failure> {
        let target = self.request_navigation(project, from, request, trace)?;

        match target {
            Navigation::Instance(instance) => {
                let module = instance.module(true)?;
                trace.candidates.insert(module.source.clone());

                Ok(module)
            }

            Navigation::File(path) => self.file_target(project, from, path, trace),
        }
    }

    fn request_navigation(
        &mut self,
        project: &mut Project,
        from: &Module,
        request: &str,
        trace: &mut Resolution,
    ) -> Result<Navigation, Failure> {
        if request.contains('\0') {
            return Err(Failure::Invalid("require path contains NUL".into()));
        }

        if let Some(instance) = &from.instance {
            trace.sourcemaps.insert(instance.map.path.clone());
        }

        let request = request.replace('\\', "/");

        let target = if let Some(aliased) = request.strip_prefix('@') {
            let (alias, rest) = aliased.split_once('/').unwrap_or((aliased, ""));

            let origin = if from.instance.is_some() {
                &from.source
            } else {
                &from.path
            };

            let scope = origin
                .parent()
                .ok_or_else(|| Failure::Invalid("module has no parent".into()))?;

            let start = self.alias(
                project,
                scope,
                &alias.to_ascii_lowercase(),
                from,
                &mut Vec::new(),
                trace,
            )?;

            self.navigate(project, start, rest, trace)?
        } else if request.starts_with("./") || request.starts_with("../") {
            let start = if let Some(instance) = &from.instance {
                Navigation::Instance(instance.parent()?)
            } else {
                Navigation::File(
                    from.path
                        .parent()
                        .ok_or_else(|| Failure::Invalid("module has no parent".into()))?
                        .to_owned(),
                )
            };

            self.navigate(project, start, &request, trace)?
        } else {
            return Err(Failure::Invalid(
                "require path must start with ./, ../, or @".into(),
            ));
        };

        Ok(target)
    }

    pub(crate) fn completions(
        &mut self,
        project: &mut Project,
        from: &Module,
        prefix: &str,
    ) -> Result<Vec<String>, Failure> {
        let prefix = if prefix.contains('\\') {
            std::borrow::Cow::Owned(prefix.replace('\\', "/"))
        } else {
            std::borrow::Cow::Borrowed(prefix)
        };

        let mut trace = Resolution {
            result: Err(Failure::Invalid(String::new())),
            candidates: BTreeSet::new(),
            configurations: BTreeSet::new(),
            sourcemaps: BTreeSet::new(),
        };

        let mut results = BTreeSet::new();

        if !prefix.contains('/') {
            let partial = prefix.to_ascii_lowercase();

            for start in ["./", "../", "@self/", "@game/"] {
                if start.starts_with(&partial)
                    && self
                        .request_navigation(project, from, start, &mut trace)
                        .is_ok()
                {
                    results.insert(start.to_owned());
                }
            }

            let config = project
                .configuration(&from.source)
                .map_err(|e| Failure::Configuration(e.to_string()))?;

            for alias in config.aliases.keys() {
                let candidate = format!("@{alias}/");

                if candidate.starts_with(&partial)
                    && self
                        .request_navigation(project, from, &candidate, &mut trace)
                        .is_ok()
                {
                    results.insert(candidate);
                }
            }

            return Ok(results.into_iter().collect());
        }

        let (directory, partial) = prefix.rsplit_once('/').expect("prefix contains slash");
        let directory = format!("{directory}/");

        let names = match self.request_navigation(project, from, &directory, &mut trace)? {
            Navigation::Instance(instance) => instance
                .child_names()
                .filter(|name| name.starts_with(partial))
                .map(|name| (name.to_owned(), true))
                .collect::<BTreeSet<_>>(),

            Navigation::File(path) => {
                let mut paths = project.overlay_children(&path).collect::<BTreeSet<_>>();

                match fs::read_dir(&path) {
                    Ok(entries) => {
                        for entry in entries {
                            paths.insert(entry.map_err(|e| Failure::Io(e.to_string()))?.path());
                        }
                    }

                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(Failure::Io(error.to_string())),
                }

                paths
                    .into_iter()
                    .filter_map(|path| {
                        let directory = path.is_dir() || project.overlay_directory(&path);

                        if !directory
                            && !matches!(
                                path.extension().and_then(|ext| ext.to_str()),
                                Some("lua" | "luau")
                            )
                        {
                            return None;
                        }

                        let name = if directory {
                            path.file_name()
                        } else {
                            path.file_stem()
                        }?
                        .to_str()?;

                        if !name.starts_with(partial) || matches!(name, "init" | ".config") {
                            return None;
                        }

                        Some((name.to_owned(), directory))
                    })
                    .collect()
            }
        };

        for (name, is_directory) in names {
            let candidate = format!("{directory}{name}");

            if self.resolve(project, from, &candidate).result.is_ok() {
                results.insert(candidate.clone());
            }

            if is_directory
                && self
                    .request_navigation(project, from, &candidate, &mut trace)
                    .is_ok()
            {
                results.insert(format!("{candidate}/"));
            }
        }

        Ok(results.into_iter().collect())
    }

    fn navigate(
        &mut self,
        project: &Project,
        start: Navigation,
        rest: &str,
        trace: &mut Resolution,
    ) -> Result<Navigation, Failure> {
        match start {
            Navigation::File(path) => self.walk(project, path, rest, trace).map(Navigation::File),

            Navigation::Instance(instance) => {
                trace.sourcemaps.insert(instance.map.path.clone());

                instance.walk(rest).map(Navigation::Instance)
            }
        }
    }

    fn file_target(
        &mut self,
        project: &mut Project,
        from: &Module,
        path: PathBuf,
        trace: &mut Resolution,
    ) -> Result<Module, Failure> {
        let source = self
            .probe(project, &path, trace)?
            .source
            .ok_or_else(|| Failure::Directory(path.clone()))?;

        if let Some(instance) = &from.instance {
            trace.sourcemaps.insert(instance.map.path.clone());

            return instance
                .map
                .find_source(&source)?
                .ok_or_else(|| {
                    Failure::Roblox(format!(
                        "{} is not mapped into the caller's sourcemap",
                        source.display()
                    ))
                })?
                .module(true);
        }

        let mut mapped = None;

        for map in project
            .sourcemaps_for(&source)
            .map_err(|e| Failure::Configuration(e.to_string()))?
        {
            trace.sourcemaps.insert(map.path.clone());

            if let Some(instance) = map.find_source(&source)? {
                if mapped.is_some() {
                    return Err(Failure::Roblox(format!(
                        "{} maps to multiple places; the caller has no place context",
                        source.display()
                    )));
                }

                mapped = Some(instance);
            }
        }

        if let Some(instance) = mapped {
            return instance.module(true);
        }

        Ok(Module {
            path,
            source,
            instance: None,
        })
    }

    fn alias(
        &mut self,
        project: &mut Project,
        scope: &Path,
        name: &str,
        from: &Module,
        stack: &mut Vec<(PathBuf, String)>,
        trace: &mut Resolution,
    ) -> Result<Navigation, Failure> {
        if name == "self" {
            return Ok(from.instance.as_ref().map_or_else(
                || Navigation::File(from.path.clone()),
                |instance| Navigation::Instance(instance.clone()),
            ));
        }

        if name == "game" {
            let map = if let Some(instance) = &from.instance {
                Some(std::rc::Rc::clone(&instance.map))
            } else {
                project
                    .sourcemap(&from.source)
                    .map_err(|e| Failure::Configuration(e.to_string()))?
            };

            if let Some(map) = map {
                trace.sourcemaps.insert(map.path.clone());

                return map.game().map(Navigation::Instance);
            }
        }

        let config = project
            .configuration_at(scope)
            .map_err(|e| Failure::Configuration(e.to_string()))?;

        trace.configurations.extend(config.inputs.iter().cloned());

        let alias = config
            .aliases
            .get(name)
            .ok_or_else(|| Failure::UnknownAlias(name.to_owned()))?;

        let key = (alias.defined_in.clone(), name.to_owned());

        if stack.contains(&key) {
            let mut cycle = stack
                .iter()
                .map(|(path, name)| format!("{}:@{name}", path.display()))
                .collect::<Vec<_>>();

            cycle.push(format!("{}:@{name}", alias.defined_in.display()));

            return Err(Failure::AliasCycle(cycle));
        }

        stack.push(key);

        let directory = alias
            .defined_in
            .parent()
            .ok_or_else(|| Failure::Invalid("alias definition has no directory".into()))?;

        let value = alias.target.replace('\\', "/");

        let result = if let Some(aliased) = value.strip_prefix('@') {
            let (next, rest) = aliased.split_once('/').unwrap_or((aliased, ""));

            let start = self.alias(
                project,
                directory,
                &next.to_ascii_lowercase(),
                from,
                stack,
                trace,
            )?;

            self.navigate(project, start, rest, trace)
        } else if value.starts_with("./") || value.starts_with("../") {
            self.walk(project, directory.to_owned(), &value, trace)
                .map(Navigation::File)
        } else if Path::new(&value).is_absolute() {
            let path = absolute(Path::new(&value)).map_err(|e| Failure::Io(e.to_string()))?;
            let path = module_path(&path);
            self.probe(project, &path, trace)?;

            Ok(Navigation::File(path))
        } else {
            Err(Failure::Invalid(format!(
                "alias @{name} target must start with ./, ../, @, or be absolute"
            )))
        };

        stack.pop();

        result
    }

    fn walk(
        &mut self,
        project: &Project,
        mut path: PathBuf,
        request: &str,
        trace: &mut Resolution,
    ) -> Result<PathBuf, Failure> {
        for part in request.split('/') {
            match part {
                "" | "." => {}

                ".." => {
                    if !path.pop() {
                        return Err(Failure::Missing(path));
                    }

                    // Like Luau, upward navigation is not ambiguous; ambiguity matters on entry.
                    if let Err(error) = self.probe(project, &path, trace)
                        && !matches!(error, Failure::Ambiguous { .. })
                    {
                        return Err(error);
                    }
                }

                ".config" => {
                    return Err(Failure::Invalid(
                        ".config.luau is not an importable module".into(),
                    ));
                }

                name => {
                    if name.contains(':') {
                        return Err(Failure::Invalid("invalid module path component".into()));
                    }

                    path.push(name);
                    self.probe(project, &path, trace)?;
                }
            }
        }

        Ok(path)
    }

    fn probe(
        &mut self,
        project: &Project,
        path: &Path,
        trace: &mut Resolution,
    ) -> Result<Entry, Failure> {
        let lookup = self.lookup(project, path);
        trace.candidates.extend(lookup.candidates);

        lookup.result
    }

    fn lookup(&mut self, project: &Project, path: &Path) -> Lookup {
        if let Some(entry) = self.entries.get(path) {
            return entry.clone();
        }

        let mut candidates = vec![path.to_owned()];

        if path.file_name().is_some_and(|name| name != "init") {
            for suffix in [".luau", ".lua"] {
                let mut name = path.as_os_str().to_os_string();
                name.push(suffix);
                candidates.push(PathBuf::from(name));
            }
        }

        candidates.push(path.join("init.luau"));
        candidates.push(path.join("init.lua"));
        let result = lookup_files(project, path, &candidates);
        let lookup = Lookup { result, candidates };
        self.entries.insert(path.to_owned(), lookup.clone());

        lookup
    }
}

fn relative_path(base: &Path, target: &Path) -> Option<String> {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = target.components().collect();

    if base.first() != target.first() {
        return None;
    }

    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();

    let parts = std::iter::repeat_n("..".to_owned(), base.len() - common)
        .chain(
            target[common..]
                .iter()
                .map(|part| part.as_os_str().to_string_lossy().into_owned()),
        )
        .collect::<Vec<_>>();

    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

fn relative_instance(base: &Instance, target: &Instance) -> Option<String> {
    let mut ancestors = Vec::new();
    let mut node = base.clone();

    loop {
        ancestors.push(node.clone());

        let Ok(parent) = node.parent() else { break };

        node = parent;
    }

    let mut names = Vec::new();
    let mut node = target.clone();

    loop {
        if let Some(index) = ancestors.iter().position(|ancestor| *ancestor == node) {
            names.reverse();

            let parts: Vec<_> = std::iter::repeat_n("..".to_owned(), index)
                .chain(names)
                .collect();

            return Some(if parts.is_empty() {
                ".".to_owned()
            } else {
                parts.join("/")
            });
        }

        if node.name().contains('/') {
            return None;
        }

        names.push(node.name().to_owned());
        node = node.parent().ok()?;
    }
}

fn lookup_files(project: &Project, path: &Path, candidates: &[PathBuf]) -> Result<Entry, Failure> {
    let mut files = Vec::new();
    let mut directory = project.overlay_directory(path);

    for candidate in candidates {
        if candidate != path && project.overlay_file(candidate) {
            files.push(candidate.clone());
            continue;
        }

        match fs::metadata(candidate) {
            Ok(metadata) if candidate == path => directory |= metadata.is_dir(),
            Ok(metadata) if metadata.is_file() => files.push(candidate.clone()),
            Ok(_) => {}

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) => {}

            Err(error) => return Err(Failure::Io(format!("{}: {error}", candidate.display()))),
        }
    }

    let sibling = files.iter().any(|file| file.parent() == path.parent());

    if files.len() > 1 || (sibling && directory) {
        if directory && sibling {
            files.push(path.to_owned());
        }

        return Err(Failure::Ambiguous {
            path: path.to_owned(),
            candidates: files,
        });
    }

    if let Some(source) = files.pop() {
        return Ok(Entry {
            source: Some(source),
        });
    }

    if directory {
        return Ok(Entry { source: None });
    }

    Err(Failure::Missing(path.to_owned()))
}

pub(crate) fn module_path(source: &Path) -> PathBuf {
    if matches!(
        source.file_name().and_then(|s| s.to_str()),
        Some("init.lua" | "init.luau")
    ) {
        source.parent().unwrap_or(source).to_owned()
    } else if matches!(
        source.extension().and_then(|s| s.to_str()),
        Some("lua" | "luau")
    ) {
        source.with_extension("")
    } else {
        source.to_owned()
    }
}
