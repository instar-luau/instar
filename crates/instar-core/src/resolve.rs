//! Abstract module navigation and cached filesystem require resolution.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
};

use serde::Serialize;

use crate::{absolute, project::Project};

/// A module's navigation identity and its separate backing source file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Module {
    /// Absolute abstract module path, without a source suffix or `init` filename.
    pub path: PathBuf,

    /// Absolute source path, preserving symlink spelling.
    pub source: PathBuf,
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

/// Filesystem navigation cache for one immutable project snapshot.
#[derive(Default)]
pub struct Resolver {
    entries: HashMap<PathBuf, Lookup>,
    resolutions: HashMap<(PathBuf, String), Resolution>,
}

impl Resolver {
    /// Creates an empty navigation cache.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps an explicitly supplied source file to its abstract module identity.
    ///
    /// # Errors
    /// Rejects missing, unsupported, or ambiguous source files.
    pub fn entry(&mut self, source: &Path) -> Result<Module, Failure> {
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

        let path = module_path(&source);
        let lookup = self.lookup(&path);
        let entry = lookup.result?;

        match entry.source {
            Some(found) if found == source => Ok(Module { path, source }),
            _ => Err(Failure::Missing(source)),
        }
    }

    /// Resolves a string require from an abstract module, caching success and failure.
    pub fn resolve(&mut self, project: &mut Project, from: &Module, request: &str) -> Resolution {
        let key = (from.path.clone(), request.to_owned());

        if let Some(result) = self.resolutions.get(&key) {
            return result.clone();
        }

        let mut trace = Resolution {
            result: Err(Failure::Invalid(String::new())),
            candidates: BTreeSet::new(),
            configurations: BTreeSet::new(),
        };

        trace.result = self.resolve_inner(project, from, request, &mut trace);
        self.resolutions.insert(key, trace.clone());

        trace
    }

    fn resolve_inner(
        &mut self,
        project: &mut Project,
        from: &Module,
        request: &str,
        trace: &mut Resolution,
    ) -> Result<Module, Failure> {
        if request.contains('\0') {
            return Err(Failure::Invalid("require path contains NUL".into()));
        }

        let request = request.replace('\\', "/");

        let path = if let Some(aliased) = request.strip_prefix('@') {
            let (alias, rest) = aliased.split_once('/').unwrap_or((aliased, ""));
            let alias = alias.to_ascii_lowercase();

            let start = if alias == "self" {
                from.path.clone()
            } else {
                let scope = from
                    .path
                    .parent()
                    .ok_or_else(|| Failure::Invalid("module has no parent".into()))?;

                self.alias(project, scope, &alias, from, &mut Vec::new(), trace)?
            };

            self.walk(start, rest, trace)?
        } else if request.starts_with("./") || request.starts_with("../") {
            let start = from
                .path
                .parent()
                .ok_or_else(|| Failure::Invalid("module has no parent".into()))?
                .to_owned();

            self.walk(start, &request, trace)?
        } else {
            return Err(Failure::Invalid(
                "require path must start with ./, ../, or @".into(),
            ));
        };

        let entry = self.probe(&path, trace)?;

        let source = entry
            .source
            .ok_or_else(|| Failure::Directory(path.clone()))?;

        Ok(Module { path, source })
    }

    fn alias(
        &mut self,
        project: &mut Project,
        scope: &Path,
        name: &str,
        from: &Module,
        stack: &mut Vec<(PathBuf, String)>,
        trace: &mut Resolution,
    ) -> Result<PathBuf, Failure> {
        if name == "self" {
            return Ok(from.path.clone());
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

            self.walk(start, rest, trace)
        } else if value.starts_with("./") || value.starts_with("../") {
            self.walk(directory.to_owned(), &value, trace)
        } else if Path::new(&value).is_absolute() {
            let path = absolute(Path::new(&value)).map_err(|e| Failure::Io(e.to_string()))?;
            let path = module_path(&path);
            self.probe(&path, trace)?;

            Ok(path)
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
                    if let Err(error) = self.probe(&path, trace)
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
                    self.probe(&path, trace)?;
                }
            }
        }

        Ok(path)
    }

    fn probe(&mut self, path: &Path, trace: &mut Resolution) -> Result<Entry, Failure> {
        let lookup = self.lookup(path);
        trace.candidates.extend(lookup.candidates);

        lookup.result
    }

    fn lookup(&mut self, path: &Path) -> Lookup {
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
        let result = lookup_files(path, &candidates);
        let lookup = Lookup { result, candidates };
        self.entries.insert(path.to_owned(), lookup.clone());

        lookup
    }
}

fn lookup_files(path: &Path, candidates: &[PathBuf]) -> Result<Entry, Failure> {
    let mut files = Vec::new();
    let mut directory = false;

    for candidate in candidates {
        match fs::metadata(candidate) {
            Ok(metadata) if candidate == path => directory = metadata.is_dir(),
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

fn module_path(source: &Path) -> PathBuf {
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
