use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use figment::{Figment, providers::Serialized};
use instar_bridge::{Language, Mode};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use super::{
    Error, Generators, assets::Assets, discovery, environment::Registry, instances, paths,
};

use crate::{
    format, lint,
    settings::{Configuration, check, defaults, resolve},
};

/// Filesystem sources with cached, automatically discovered configuration scopes.
///
/// Module identifiers are canonical UTF-8 paths accepted by [`instar_bridge::Frontend`].
pub struct Workspace {
    pub(super) root: PathBuf,
    pub(super) files: Vec<PathBuf>,
    pub(super) overrides: Vec<String>,
    pub(super) scopes: RefCell<BTreeMap<PathBuf, Rc<Scope>>>,
    pub(super) configuration_paths: RefCell<BTreeSet<PathBuf>>,
    pub(super) instances: RefCell<instances::Registry>,
    pub(super) sources: RefCell<BTreeMap<String, Arc<str>>>,
    pub(super) snapshots: RefCell<BTreeMap<String, Option<Arc<str>>>>,
    pub(super) observed_paths: RefCell<BTreeSet<PathBuf>>,
    pub(super) assets: Assets,
    pub(super) environments: RefCell<Registry>,
    pub(super) cancellation: RefCell<CancellationToken>,
    pub(super) generation: Option<Generators>,
}

impl Workspace {
    /// Open a workspace, applying JSON overrides after its TOML layers.
    ///
    /// Discovers ancestor and nested project and native language configuration files.
    /// Explicit files replace project configuration discovery in the supplied order.
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
            configuration_paths: RefCell::new(BTreeSet::new()),
            instances: RefCell::new(instances::Registry::default()),
            sources: RefCell::new(BTreeMap::new()),
            snapshots: RefCell::new(BTreeMap::new()),
            observed_paths: RefCell::new(BTreeSet::new()),
            assets: Assets::default(),
            environments: RefCell::new(Registry::default()),
            cancellation: RefCell::new(CancellationToken::new()),
            generation: None,
        };

        workspace.reload()?;

        Ok(workspace)
    }

    /// Use the supplied token to cancel background workspace operations.
    #[must_use]
    pub fn with_cancellation(self, cancellation: CancellationToken) -> Self {
        self.set_cancellation(cancellation);

        self
    }

    /// Replace the token captured by subsequent synchronous workspace operations.
    pub fn set_cancellation(&self, cancellation: CancellationToken) {
        *self.cancellation.borrow_mut() = cancellation;
    }

    /// Share persistent sourcemap producers with this workspace.
    #[must_use]
    pub fn with_generators(mut self, generators: Generators) -> Self {
        self.set_generators(generators);

        self
    }

    /// Replace the persistent sourcemap producers and invalidate cached instance maps.
    pub fn set_generators(&mut self, generators: Generators) {
        self.generation = Some(generators);
        *self.instances.get_mut() = instances::Registry::default();
    }

    /// Reload configuration, preserving the previous scopes if the root cannot load.
    ///
    /// # Errors
    /// Returns configuration, filesystem, or pattern failures.
    pub fn reload(&mut self) -> Result<(), Error> {
        let scope = discovery::root(self)?;
        *self.scopes.get_mut() = BTreeMap::from([(self.root.clone(), Rc::new(scope))]);
        self.configuration_paths.get_mut().clear();
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

        dependencies.extend(
            self.scopes
                .borrow()
                .values()
                .flat_map(|scope| scope.configuration.resolve.aliases.values().cloned()),
        );

        dependencies
    }

    /// Return configuration and environment inputs whose changes invalidate native state.
    #[must_use]
    pub fn reload_dependencies(&self) -> BTreeSet<PathBuf> {
        let mut dependencies: BTreeSet<_> = discovery::configuration_files(&self.root, &self.files)
            .into_iter()
            .collect();

        let scopes = self.scopes.borrow();

        dependencies.extend(self.configuration_paths.borrow().iter().cloned());

        for scope in scopes.values() {
            let configuration = &scope.configuration;
            dependencies.extend(configuration.resolve.sourcemaps.iter().flatten().cloned());
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

    /// Normalize a path relative to the root without accessing the filesystem.
    ///
    /// # Errors
    /// Returns non-UTF-8 paths.
    pub fn normalize(&self, path: &Path) -> Result<String, Error> {
        paths::text(&self.root.join(path))
    }

    /// Identify a disk or overlaid `.luau` or `.lua` source relative to the workspace root.
    ///
    /// # Errors
    /// Returns inaccessible, non-UTF-8, or unsupported source paths.
    pub fn module(&self, path: &Path) -> Result<String, Error> {
        let path = paths::identity(&self.root.join(path))?;
        let module = paths::text(&path)?;

        if !paths::is_source(&path)
            || !(self.sources.borrow().contains_key(&module)
                || paths::metadata(&path)?.is_some_and(|metadata| metadata.is_file()))
        {
            return Err(Error::Path {
                path,
                reason: "Expected a `.luau` or `.lua` source file.",
            });
        }

        Ok(module)
    }

    /// Return the effective formatting configuration for a module.
    ///
    /// # Errors
    /// Returns inaccessible configuration or invalid scoped settings.
    pub fn formatting(&self, module: &str) -> Result<Arc<format::Prepared>, Error> {
        Ok(Arc::clone(&self.scope(Path::new(module))?.format))
    }

    /// Return the effective lint policies for a module.
    ///
    /// # Errors
    /// Returns inaccessible configuration or invalid scoped settings.
    pub fn linting(&self, module: &str) -> Result<Arc<lint::Prepared>, Error> {
        Ok(Arc::clone(&self.scope(Path::new(module))?.lint))
    }

    pub(super) fn scope(&self, module: &Path) -> Result<Rc<Scope>, Error> {
        discovery::directory(self, module.parent().unwrap_or(&self.root))
    }

    pub(super) fn root(&self) -> Rc<Scope> {
        Rc::clone(&self.scopes.borrow()[&self.root])
    }
}

pub(super) struct Scope {
    pub(super) layers: Figment,
    pub(super) language: Option<Language>,
    pub(super) configuration: Configuration,
    pub(super) format: Arc<format::Prepared>,
    pub(super) lint: Arc<lint::Prepared>,
    pub(super) symbols: RefCell<BTreeMap<Option<String>, super::SymbolOutcome>>,
}

impl Scope {
    pub(super) fn new(
        base: &Path,
        layers: Figment,
        language: Option<Language>,
        overrides: &[String],
    ) -> Result<Self, Error> {
        let mut settings = defaults();

        if let Some(language) = &language {
            let mode = match language.mode() {
                Mode::Strict => check::Mode::Strict,
                Mode::NonStrict => check::Mode::NonStrict,
                Mode::NoCheck => check::Mode::NoCheck,
            };

            settings = settings.merge(Serialized::defaults(json!({
                "check": { "mode": mode },
                "environment": { "globals": language.globals() },
                "resolve": { "aliases": language.aliases() },
            })));
        }

        let configuration = resolve(base, settings.merge(layers.clone()), overrides)
            .map_err(Error::Configuration)?;

        let format = format::Prepared::new(&configuration.format).map_err(|mut error| {
            error.path.insert(0, "format".to_owned());

            Error::Configuration(error)
        })?;

        let lint = lint::Prepared::new(&configuration.lint, configuration.format.indent_width)
            .map_err(|mut error| {
                error.path.insert(0, "lint".to_owned());

                Error::Configuration(error)
            })?;

        Ok(Self {
            layers,
            language,
            configuration,
            format: Arc::new(format),
            lint: Arc::new(lint),
            symbols: RefCell::default(),
        })
    }
}
