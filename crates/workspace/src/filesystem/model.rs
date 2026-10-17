use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use figment::Figment;
use instar_configuration::{Configuration, formatting, resolve};
use instar_lint::Symbols;
use instar_luau::Context;
use serde_json::Value;

use super::{
    Error, discovery,
    environment::{self, Registry},
    host, instances, paths,
    selection::{self, Access, Purpose, Selection},
};

/// Filesystem sources with cached, automatically discovered configuration scopes.
///
/// Module identifiers are canonical UTF-8 paths accepted by [`instar_luau::Frontend`].
pub struct Workspace {
    pub(super) root: PathBuf,
    pub(super) files: Vec<PathBuf>,
    pub(super) overrides: Vec<String>,
    pub(super) scopes: RefCell<BTreeMap<PathBuf, Rc<Scope>>>,
    pub(super) instances: RefCell<instances::Registry>,
    pub(super) sources: BTreeMap<String, Arc<str>>,
    pub(super) snapshots: RefCell<BTreeMap<String, Option<Arc<str>>>>,
    pub(super) observed_paths: RefCell<BTreeSet<PathBuf>>,
    pub(super) environments: RefCell<Registry>,
}

pub(super) struct Scope {
    pub(super) layers: Figment,
    pub(super) configuration: Configuration,
    pub(super) checking: Selection,
    pub(super) formatting: Selection,
    pub(super) format: Arc<formatting::Prepared>,
    pub(super) linting: Selection,
    pub(super) lint: Arc<instar_lint::Prepared>,
    pub(super) environments: RefCell<BTreeMap<bool, Option<String>>>,
    builtins: RefCell<Option<Arc<str>>>,
}

impl Workspace {
    /// Open a workspace, applying JSON overrides after its TOML layers.
    ///
    /// An empty file list discovers ancestor and nested `instar.toml` files.
    /// Explicit files replace discovery and apply in the supplied order.
    /// Relative explicit files and override paths use the workspace root.
    ///
    /// # Errors
    /// Returns inaccessible paths, configuration errors, or malformed patterns.
    pub fn new(root: &Path, files: &[PathBuf], overrides: &[String]) -> Result<Self, Error> {
        let root = paths::directory(root)?;

        let mut workspace = Self {
            root,
            files: files.to_owned(),
            overrides: overrides.to_owned(),
            scopes: RefCell::new(BTreeMap::new()),
            instances: RefCell::new(instances::Registry::default()),
            sources: BTreeMap::new(),
            snapshots: RefCell::new(BTreeMap::new()),
            observed_paths: RefCell::new(BTreeSet::new()),
            environments: RefCell::new(Registry::default()),
        };

        workspace.reload()?;

        Ok(workspace)
    }

    /// Reload configuration, preserving the previous scopes if the root cannot load.
    ///
    /// # Errors
    /// Returns configuration, filesystem, or pattern failures.
    pub fn reload(&mut self) -> Result<(), Error> {
        let scope = discovery::root(self)?;
        *self.scopes.get_mut() = BTreeMap::from([(self.root.clone(), Rc::new(scope))]);
        *self.instances.get_mut() = instances::Registry::default();
        *self.environments.get_mut() = Registry::default();
        self.snapshots.get_mut().clear();
        self.observed_paths.get_mut().clear();

        Ok(())
    }

    /// Return the canonical project directory.
    pub fn directory(&self) -> &Path {
        &self.root
    }

    /// Return observed paths and effective configuration, resolution, and environment inputs.
    ///
    /// Includes source requests, navigation targets, missing discovery candidates, and configured paths.
    /// Does not access the filesystem. Observations accumulate until a successful reload.
    pub fn dependencies(&self) -> BTreeSet<PathBuf> {
        let mut dependencies = self.reload_dependencies();
        dependencies.extend(self.observed_paths.borrow().iter().cloned());

        dependencies
    }

    /// Identify configuration, resolution, and environment inputs that require a full reload.
    ///
    /// Existing dependency paths are compared by canonical identity; missing candidates are skipped.
    ///
    /// # Errors
    /// Returns inaccessible paths or an unavailable module identity.
    pub fn requires_reload(&self, module: &str) -> Result<bool, Error> {
        let module = paths::canonical(Path::new(module))?;

        for path in self.reload_dependencies() {
            if paths::metadata(&path)?.is_some() && paths::canonical(&path)? == module {
                return Ok(true);
            }
        }

        Ok(false)
    }

    /// Identify an existing `.luau` or `.lua` source relative to the workspace root.
    ///
    /// # Errors
    /// Returns inaccessible, non-UTF-8, or unsupported source paths.
    pub fn module(&self, path: &Path) -> Result<String, Error> {
        let path = paths::canonical(&self.root.join(path))?;

        if !paths::source(&path)
            || !paths::metadata(&path)?.is_some_and(|metadata| metadata.is_file())
        {
            return Err(Error::Path {
                path,
                reason: "expected a .luau or .lua source file",
            });
        }

        paths::text(&path)
    }

    /// Map sorted, unique modules to their first canonical input; an empty list traverses the root.
    ///
    /// Explicit files are selected directly. Directories and glob patterns apply
    /// command inclusion/exclusion settings, falling back to shared settings.
    /// Traversal does not follow symlinks. Glob inputs use their fixed directory prefix.
    ///
    /// # Errors
    /// Returns malformed patterns, inaccessible inputs, and unsupported explicit files.
    pub fn select(
        &self,
        inputs: &[PathBuf],
        purpose: Purpose,
        access: Access,
    ) -> Result<BTreeMap<String, PathBuf>, Error> {
        selection::select(self, inputs, purpose, access)
    }

    /// Return the effective formatting configuration for a module.
    ///
    /// # Errors
    /// Returns inaccessible configuration or invalid scoped settings.
    pub fn formatting(&self, module: &str) -> Result<Arc<formatting::Prepared>, Error> {
        Ok(Arc::clone(&self.scope(Path::new(module))?.format))
    }

    /// Return the effective lint policies for a module.
    ///
    /// # Errors
    /// Returns inaccessible configuration or invalid scoped settings.
    pub fn linting(&self, module: &str) -> Result<Arc<instar_lint::Prepared>, Error> {
        Ok(Arc::clone(&self.scope(Path::new(module))?.lint))
    }

    /// Return declared value and type names for a module's lint environment.
    ///
    /// # Errors
    /// Returns invalid flags, declaration syntax, or environment inputs.
    pub fn symbols(&self, module: &str) -> Result<Arc<Symbols>, Error> {
        let scope = self.scope(Path::new(module))?;
        let cached = scope.builtins.borrow().as_ref().map(Arc::clone);

        let builtins = if let Some(builtins) = cached {
            builtins
        } else {
            let builtins: Arc<str> = Context::new(self)
                .builtins(host::flags(&scope.configuration))
                .map_err(Error::Analysis)?
                .into();

            *scope.builtins.borrow_mut() = Some(Arc::clone(&builtins));

            builtins
        };

        let name = environment::identity(self, &scope, Path::new(module))?;

        self.environments.borrow_mut().symbols(
            name.as_deref(),
            &scope.configuration.environment.globals,
            &builtins,
        )
    }

    /// Look up a configured documentation symbol in the module's environment.
    ///
    /// # Errors
    /// Returns unavailable, malformed, or inconsistent environment inputs.
    pub fn documentation(&self, module: &str, symbol: &str) -> Result<Option<Value>, Error> {
        let scope = self.scope(Path::new(module))?;
        let name = environment::identity(self, &scope, Path::new(module))?;
        let mut registry = self.environments.borrow_mut();

        let documentation = registry
            .documents(&scope.configuration.environment.groups)?
            .iter()
            .rev()
            .find_map(|documentation| documentation.get(symbol).cloned());

        Ok(documentation.or_else(|| {
            name.as_deref()
                .and_then(|name| registry.get(name))
                .and_then(|prepared| prepared.documentation.as_ref())
                .and_then(|documentation| documentation.get(symbol).cloned())
        }))
    }

    fn reload_dependencies(&self) -> BTreeSet<PathBuf> {
        let mut dependencies = BTreeSet::new();
        let scopes = self.scopes.borrow();

        if self.files.is_empty() {
            dependencies.extend(
                self.root
                    .ancestors()
                    .map(|directory| directory.join("instar.toml")),
            );

            dependencies.extend(scopes.keys().map(|directory| directory.join("instar.toml")));
        } else {
            dependencies.extend(self.files.iter().map(|file| self.root.join(file)));
        }

        for scope in scopes.values() {
            let configuration = &scope.configuration;
            dependencies.extend(configuration.resolve.aliases.values().cloned());
            dependencies.extend(configuration.resolve.sourcemaps.iter().flatten().cloned());
            dependencies.extend(configuration.environment.asset_directory.iter().cloned());
            dependencies.extend(configuration.environment.capabilities.iter().cloned());

            for group in &configuration.environment.groups {
                dependencies.extend(group.definitions.iter().cloned());
                dependencies.extend(group.documentation.iter().cloned());
            }
        }

        self.instances.borrow().dependencies(&mut dependencies);
        self.environments.borrow().dependencies(&mut dependencies);

        dependencies
    }

    pub(super) fn scope(&self, module: &Path) -> Result<Rc<Scope>, Error> {
        discovery::directory(self, module.parent().unwrap_or(&self.root))
    }

    pub(super) fn root(&self) -> Rc<Scope> {
        Rc::clone(&self.scopes.borrow()[&self.root])
    }
}

impl Scope {
    pub(super) fn new(base: &Path, layers: Figment, overrides: &[String]) -> Result<Self, Error> {
        let configuration =
            resolve(base, layers.clone(), overrides).map_err(Error::Configuration)?;

        let format = configuration
            .format
            .clone()
            .prepare()
            .map_err(|mut error| {
                error.path.insert(0, "format".to_owned());

                Error::Configuration(error)
            })?;

        let lint = instar_lint::Prepared::new(&configuration.lint).map_err(|mut error| {
            error.path.insert(0, "lint".to_owned());

            Error::Configuration(error)
        })?;

        let checking = Selection::new(
            base,
            configuration
                .check
                .include
                .as_deref()
                .or(configuration.include.as_deref()),
            configuration
                .check
                .exclude
                .as_deref()
                .or(configuration.exclude.as_deref()),
        )?;

        let formatting = Selection::new(
            base,
            configuration
                .format
                .include
                .as_deref()
                .or(configuration.include.as_deref()),
            configuration
                .format
                .exclude
                .as_deref()
                .or(configuration.exclude.as_deref()),
        )?;

        let linting = Selection::new(
            base,
            configuration
                .lint
                .include
                .as_deref()
                .or(configuration.include.as_deref()),
            configuration
                .lint
                .exclude
                .as_deref()
                .or(configuration.exclude.as_deref()),
        )?;

        Ok(Self {
            layers,
            configuration,
            checking,
            formatting,
            format: Arc::new(format),
            linting,
            lint: Arc::new(lint),
            environments: RefCell::new(BTreeMap::new()),
            builtins: RefCell::new(None),
        })
    }
}
