//! Cached filesystem and sourcemap-backed require resolution.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
};

use serde::Serialize;

use crate::{
    absolute,
    config::RequireStyle,
    identifier,
    project::Project,
    roblox::{Instance, Sourcemap},
};

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

    /// Returns mapped `ModuleScript` instances in the caller's active place contexts.
    /// `None` means no sourcemap exists; an empty list means no safe mapped candidates.
    ///
    /// # Errors
    /// Returns configuration or filesystem errors without parsing module sources.
    pub fn import_candidates(
        &mut self,
        project: &mut Project,
        source: &Path,
    ) -> Result<Option<Vec<Module>>, Failure> {
        let source = absolute(source).map_err(|error| Failure::Io(error.to_string()))?;

        let Some(maps) = import_maps(project, &source)? else {
            return Ok(None);
        };

        let mut modules = Vec::new();

        for map in maps {
            for module in map.modules() {
                if project.overlay_file(&module.source) {
                    modules.push(module);
                    continue;
                }

                match fs::metadata(&module.source) {
                    Ok(metadata) if metadata.is_file() => modules.push(module),
                    Ok(_) => {}

                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) => {}

                    Err(error) => return Err(Failure::Io(error.to_string())),
                }
            }
        }

        Ok(Some(modules))
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
        style: RequireStyle,
        services: &[String],
    ) -> Result<Option<String>, Failure> {
        let source = absolute(source).map_err(|error| Failure::Io(error.to_string()))?;
        let target = absolute(target).map_err(|error| Failure::Io(error.to_string()))?;
        let origins = self.entries(project, &source)?;
        let maps = import_maps(project, &source)?.unwrap_or_default();
        let mut mapped = false;
        let mut targets = Vec::new();

        for map in &maps {
            for instance in map.instances_for_source(&target) {
                mapped = true;

                if let Ok(module) = instance.module(true) {
                    targets.push(module);
                }
            }
        }

        if mapped && targets.is_empty() {
            return Ok(None);
        }

        if !mapped {
            targets = self.entries(project, &target)?;
        }

        if style == RequireStyle::Instance && targets.iter().any(|target| target.instance.is_some())
        {
            return Ok(import_instance_argument(
                &origins, &targets, &maps, services,
            ));
        }

        let config = project
            .configuration(&source)
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
                        (Navigation::File(base), _) => relative_path(base, &destination.path),

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
                return Ok(Some(string_literal(&request)));
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

fn import_maps(
    project: &mut Project,
    source: &Path,
) -> Result<Option<Vec<Rc<Sourcemap>>>, Failure> {
    let maps = project
        .sourcemaps_for(source)
        .map_err(|error| Failure::Configuration(error.to_string()))?;

    if maps.is_empty() {
        return Ok(None);
    }

    let matching: Vec<_> = maps
        .iter()
        .filter(|map| map.instances_for_source(source).next().is_some())
        .cloned()
        .collect();

    Ok(Some(if !matching.is_empty() {
        matching
    } else if maps.len() == 1 {
        maps
    } else {
        Vec::new()
    }))
}

fn import_instance_argument(
    origins: &[Module],
    targets: &[Module],
    maps: &[Rc<Sourcemap>],
    services: &[String],
) -> Option<String> {
    let mut common: Option<BTreeSet<String>> = None;

    for origin in origins {
        let map = origin
            .instance
            .as_ref()
            .map(|instance| &instance.map)
            .or_else(|| (maps.len() == 1).then(|| &maps[0]))?;

        let mut expressions = BTreeSet::new();

        for target in targets {
            let Some(instance) = &target.instance else {
                continue;
            };

            if !Rc::ptr_eq(&instance.map, map) {
                continue;
            }

            if let Ok(game) = map.game()
                && let Some(expression) = instance_expression(&game, instance, "game", services)
            {
                expressions.insert(expression);
            }

            if let Some(script) = &origin.instance
                && let Some(expression) = instance_expression(script, instance, "script", services)
            {
                expressions.insert(expression);
            }
        }

        // Only expressions known to reach this physical target in every caller survive.
        common = Some(match common {
            None => expressions,
            Some(previous) => previous.intersection(&expressions).cloned().collect(),
        });
    }

    common?.into_iter().min_by_key(|expression| {
        (
            !expression.starts_with("game"),
            expression.len(),
            expression.clone(),
        )
    })
}

fn string_literal(value: &str) -> String {
    let mut literal = String::from("\"");

    for character in value.chars() {
        match character {
            '"' => literal.push_str("\\\""),
            '\\' => literal.push_str("\\\\"),
            '\n' => literal.push_str("\\n"),
            '\r' => literal.push_str("\\r"),
            '\t' => literal.push_str("\\t"),
            character if character.is_control() => literal.extend(character.escape_unicode()),
            character => literal.push(character),
        }
    }

    literal.push('"');

    literal
}

fn instance_expression(
    base: &Instance,
    target: &Instance,
    root: &str,
    services: &[String],
) -> Option<String> {
    let (up, children) = instance_path(base, target)?;
    let mut expression = root.to_owned();

    for _ in 0..up {
        expression.push_str(".Parent");
    }

    for child in children {
        let name = child.name();

        if expression == "game" && services.iter().any(|service| service == child.class_name()) {
            if base.service(child.class_name()).ok()? != child {
                return None;
            }

            expression.push_str(":GetService(");
            expression.push_str(&string_literal(child.class_name()));
            expression.push(')');
            continue;
        }

        // ponytail: inherited members only; consult class metadata for class-specific collisions.
        if !identifier(name)
            || matches!(
                name,
                "Parent"
                    | "Name"
                    | "ClassName"
                    | "Archivable"
                    | "RobloxLocked"
                    | "UniqueId"
                    | "Capabilities"
                    | "DefinesCapabilities"
                    | "Sandboxed"
                    | "AncestryChanged"
                    | "AttributeChanged"
                    | "Changed"
                    | "ChildAdded"
                    | "ChildRemoved"
                    | "DescendantAdded"
                    | "DescendantRemoving"
                    | "Destroying"
                    | "AddTag"
                    | "ClearAllChildren"
                    | "Clone"
                    | "Destroy"
                    | "FindFirstAncestor"
                    | "FindFirstAncestorOfClass"
                    | "FindFirstAncestorWhichIsA"
                    | "FindFirstChild"
                    | "FindFirstChildOfClass"
                    | "FindFirstChildWhichIsA"
                    | "FindFirstDescendant"
                    | "GetActor"
                    | "GetAttribute"
                    | "GetAttributes"
                    | "GetAttributeChangedSignal"
                    | "GetChildren"
                    | "GetDebugId"
                    | "GetDescendants"
                    | "GetFullName"
                    | "GetPropertyChangedSignal"
                    | "GetService"
                    | "GetTags"
                    | "HasTag"
                    | "IsA"
                    | "IsAncestorOf"
                    | "IsDescendantOf"
                    | "RemoveTag"
                    | "SetAttribute"
                    | "WaitForChild"
            )
        {
            expression.push_str(":WaitForChild(");
            expression.push_str(&string_literal(name));
            expression.push(')');
        } else {
            expression.push('.');
            expression.push_str(name);
        }
    }

    Some(expression)
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
    let (up, children) = instance_path(base, target)?;
    let mut parts: Vec<_> = std::iter::repeat_n("..".to_owned(), up).collect();

    for child in children {
        let name = child.name();

        if matches!(name, "" | "." | "..") || name.contains(['/', '\\']) {
            return None;
        }

        parts.push(name.to_owned());
    }

    Some(if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    })
}

fn instance_path(base: &Instance, target: &Instance) -> Option<(usize, Vec<Instance>)> {
    let mut ancestors = Vec::new();
    let mut node = base.clone();

    loop {
        ancestors.push(node.clone());

        let Ok(parent) = node.parent() else { break };

        node = parent;
    }

    let mut children = Vec::new();
    let mut node = target.clone();

    loop {
        if let Some(index) = ancestors.iter().position(|ancestor| *ancestor == node) {
            children.reverse();

            return Some((index, children));
        }

        let parent = node.parent().ok()?;

        if parent.child(node.name()).ok()? != node {
            return None;
        }

        children.push(node);
        node = parent;
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture(name: &str, maps: &[&str]) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("resolve-imports-{name}-{}", std::process::id()));

        fs::create_dir_all(root.join("place")).unwrap();

        fs::write(
            root.join("instar.toml"),
            format!(
                "[roblox]\nsourcemaps = {}\n",
                serde_json::to_string(maps).unwrap()
            ),
        )
        .unwrap();

        root
    }

    fn write_map(root: &Path, name: &str, children: &serde_json::Value) {
        fs::write(
            root.join(name),
            json!({
                "name": "Place", "className": "DataModel", "children": children,
            })
            .to_string(),
        )
        .unwrap();
    }

    #[test]
    fn instance_imports_use_service_classes_and_preserve_other_paths() {
        let root = fixture("services", &["place/a.json"]);
        let source = root.join("source.luau");
        let target = root.join("module.luau");
        let services = ["ReplicatedStorage".into()];

        for (class, name, expected) in [
            (
                "ReplicatedStorage",
                "ReplicatedStorage",
                "game:GetService(\"ReplicatedStorage\").packages.Module",
            ),
            (
                "ReplicatedStorage",
                "Renamed",
                "game:GetService(\"ReplicatedStorage\").packages.Module",
            ),
            (
                "Folder",
                "ReplicatedStorage",
                "game.ReplicatedStorage.packages.Module",
            ),
        ] {
            write_map(
                &root,
                "place/a.json",
                &json!([
                    {"name": "Caller", "className": "Script", "filePaths": ["../source.luau"]},
                    {"name": name, "className": class, "children": [
                        {"name": "packages", "className": "Folder", "children": [
                            {"name": "Module", "className": "ModuleScript", "filePaths": ["../module.luau"]}
                        ]}
                    ]}
                ]),
            );

            let mut project = Project::new();
            project.set_source(&source, Some("return 1")).unwrap();
            project.set_source(&target, Some("return {}")).unwrap();
            let mut resolver = Resolver::new();

            let argument = resolver
                .import_argument(
                    &mut project,
                    &source,
                    &target,
                    RequireStyle::Instance,
                    &services,
                )
                .unwrap()
                .unwrap();

            assert_eq!(argument, expected);

            assert_eq!(
                resolver
                    .import_argument(
                        &mut project,
                        &source,
                        &target,
                        RequireStyle::String,
                        &services
                    )
                    .unwrap(),
                Some(format!("\"@game/{name}/packages/Module\""))
            );

            project
                .set_source(&source, Some(&format!("return require({argument})")))
                .unwrap();

            let links = project.links(&source).unwrap();
            assert_eq!(links.len(), 1);
            assert_eq!(links[0].1, target);
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mapped_candidates_and_arguments_handle_names_without_parsing_sources() {
        let root = fixture("names", &["place/a.json", "place/b.json"]);
        let source = root.join("shared/source.luau");
        let mut project = Project::new();
        project.set_source(&source, Some("return 1")).unwrap();

        let mut children = vec![json!({
            "name": "Caller", "className": "Script", "filePaths": ["../shared/source.luau"]
        })];

        let names = [
            "Util",
            "end",
            "bad-name",
            "a/b",
            "a\\b",
            ".",
            "",
            "Name",
            "Clone",
            "Destroy",
            "Archivable",
            "a\"\n\u{1}",
        ];

        for (index, name) in names.iter().enumerate() {
            let file = format!("../shared/module{index}.luau");

            project
                .set_source(&root.join("place").join(&file), Some("not valid Luau !!!"))
                .unwrap();

            children.push(json!({"name": name, "className": "ModuleScript", "filePaths": [file]}));
        }

        children.extend([
            json!({"name": "Missing", "className": "ModuleScript", "filePaths": ["missing.luau"]}),
            json!({"name": "NoSource", "className": "ModuleScript"}),
            json!({"name": "ManySources", "className": "ModuleScript", "filePaths": ["a.luau", "b.luau"]}),
            json!({"name": "Duplicate", "className": "ModuleScript", "filePaths": ["../shared/module0.luau"]}),
            json!({"name": "Duplicate", "className": "Folder"}),
        ]);

        write_map(&root, "place/a.json", &json!(children));

        write_map(
            &root,
            "place/b.json",
            &json!([
                {"name": "Foreign", "className": "ModuleScript", "filePaths": ["../shared/module0.luau"]}
            ]),
        );

        let mut resolver = Resolver::new();

        let candidates = resolver
            .import_candidates(&mut project, &source)
            .unwrap()
            .unwrap();

        assert_eq!(candidates.len(), names.len());

        assert!(
            candidates
                .iter()
                .all(|module| module.instance.as_ref().unwrap().sourcemap_path()
                    == root.join("place/a.json"))
        );

        assert_eq!(
            resolver
                .import_candidates(&mut project, &root.join("unmapped.luau"))
                .unwrap()
                .unwrap(),
            []
        );

        for (index, name) in names.iter().enumerate() {
            let target = root.join(format!("shared/module{index}.luau"));

            let expression = resolver
                .import_argument(&mut project, &source, &target, RequireStyle::Instance, &[])
                .unwrap()
                .unwrap();

            if *name == "Util" {
                assert_eq!(expression, "game.Util");

                assert_eq!(
                    resolver
                        .import_argument(&mut project, &source, &target, RequireStyle::String, &[])
                        .unwrap(),
                    Some("\"@game/Util\"".into())
                );
            } else {
                assert!(expression.starts_with("game:WaitForChild("), "{expression}");
            }

            project
                .set_source(&source, Some(&format!("return require({expression})")))
                .unwrap();

            let mut graph = crate::graph::Graph::new();

            graph
                .add_entries(&mut project, std::slice::from_ref(&source))
                .unwrap();

            let entry = &graph.nodes[*graph.entries.first().unwrap()];
            assert!(entry.requires[0].failure.is_none(), "{expression}");

            assert_eq!(
                graph.nodes[entry.requires[0].target.unwrap()].module.source,
                target
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shared_caller_requires_one_safe_argument_across_places() {
        let root = fixture("shared", &["place/a.json", "place/b.json"]);
        let source = root.join("shared/source.luau");
        let target = root.join("shared/module.luau");
        let only_first = root.join("shared/only-first.luau");
        let mut project = Project::new();

        for path in [&source, &target, &only_first] {
            project.set_source(path, Some("return 1")).unwrap();
        }

        for (map, folder) in [("place/a.json", "A"), ("place/b.json", "B")] {
            let mut children = vec![
                json!({"name": "Caller", "className": "Script", "filePaths": ["../shared/source.luau"]}),
                json!({"name": "Util", "className": "ModuleScript", "filePaths": ["../shared/module.luau"]}),
            ];

            if folder == "A" {
                children.push(json!({"name": "OnlyFirst", "className": "ModuleScript", "filePaths": ["../shared/only-first.luau"]}));
            }

            write_map(
                &root,
                map,
                &json!([{"name": folder, "className": "Folder", "children": children}]),
            );
        }

        let mut resolver = Resolver::new();

        assert_eq!(
            resolver
                .import_candidates(&mut project, &source)
                .unwrap()
                .unwrap()
                .len(),
            3
        );

        assert_eq!(
            resolver
                .import_argument(&mut project, &source, &target, RequireStyle::Instance, &[])
                .unwrap(),
            Some("script.Parent.Util".into())
        );

        assert_eq!(
            resolver
                .import_argument(&mut project, &source, &target, RequireStyle::String, &[])
                .unwrap(),
            Some("\"./Util\"".into())
        );

        for style in [RequireStyle::Instance, RequireStyle::String] {
            assert_eq!(
                resolver
                    .import_argument(&mut project, &source, &only_first, style, &[])
                    .unwrap(),
                None
            );
        }

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn filesystem_imports_stay_strings_and_a_single_map_supports_unmapped_callers() {
        let root = fixture("filesystem", &[]);
        let source = root.join("source.luau");
        let target = root.join("module.luau");
        let mut project = Project::new();

        for path in [&source, &target] {
            project.set_source(path, Some("return 1")).unwrap();
        }

        let mut resolver = Resolver::new();

        assert!(
            resolver
                .import_candidates(&mut project, &source)
                .unwrap()
                .is_none()
        );

        for style in [RequireStyle::Instance, RequireStyle::String] {
            assert_eq!(
                resolver
                    .import_argument(&mut project, &source, &target, style, &[])
                    .unwrap(),
                Some("\"./module\"".into())
            );
        }

        write_map(
            &root,
            "place/a.json",
            &json!([
                {"name": "Util", "className": "ModuleScript", "filePaths": ["../module.luau"]}
            ]),
        );

        fs::write(
            root.join("instar.toml"),
            "[roblox]\nsourcemaps = [\"place/a.json\"]\n[luau.aliases]\nu = \"@game/Util\"\n",
        )
        .unwrap();

        let mut mapped = Project::new();

        for path in [&source, &target] {
            mapped.set_source(path, Some("return 1")).unwrap();
        }

        let mut resolver = Resolver::new();

        assert_eq!(
            resolver
                .import_candidates(&mut mapped, &source)
                .unwrap()
                .unwrap()
                .len(),
            1
        );

        assert_eq!(
            resolver
                .import_argument(&mut mapped, &source, &target, RequireStyle::Instance, &[])
                .unwrap(),
            Some("game.Util".into())
        );

        assert_eq!(
            resolver
                .import_argument(&mut mapped, &source, &target, RequireStyle::String, &[])
                .unwrap(),
            Some("\"@u\"".into())
        );

        fs::remove_dir_all(root).unwrap();
    }
}
