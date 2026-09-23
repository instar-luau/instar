//! Project configuration discovery and source snapshots.

use std::{
    cell::OnceCell,
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::{
    absolute,
    assets::{self, Assets, Definitions},
    config::{self, Config, FormatOptions, LuauConfig, RobloxConfig, Security},
    filter::{Filters, Service},
    invalid,
    roblox::{Sourcemap, SourcemapLocation},
};

/// An alias with the configuration file that defines its lookup scope.
#[derive(Clone, Debug)]
pub struct Alias {
    /// Target expression or filesystem path.
    pub target: String,

    /// Defining configuration file, retained even after inheritance.
    pub defined_in: PathBuf,
}

/// Effective configuration for a directory in a project snapshot.
pub struct EffectiveConfig {
    /// Merged settings, with Instar values taking precedence over legacy values.
    pub settings: LuauConfig,

    /// Effective formatting settings for the source directory.
    pub format_options: FormatOptions,

    /// Alias definitions, indexed by lowercase name.
    pub aliases: BTreeMap<String, Alias>,

    /// Native-compatible JSON representation of the effective settings.
    pub json: String,

    /// Configuration lookup dependencies, including files that did not exist.
    pub inputs: Vec<PathBuf>,

    /// Effective sourcemaps, retaining their explicit or automatically discovered origin.
    pub sourcemaps: Vec<SourcemapLocation>,

    /// Effective platform detection and asset locations.
    pub roblox: RobloxSettings,

    pub(crate) definition_order: Vec<String>,

    native: instar_bridge::Configuration,
    without_lints: OnceCell<instar_bridge::Configuration>,
    filters: Filters,
}

impl EffectiveConfig {
    /// Borrows the native configuration built from the merged JSON.
    #[must_use]
    pub const fn native(&self) -> &instar_bridge::Configuration {
        &self.native
    }

    pub(crate) fn analysis_native(
        &self,
        source: &Path,
        service: Service,
        lint: bool,
    ) -> io::Result<&instar_bridge::Configuration> {
        if lint
            && self.filters.includes(source, service)
            && self.filters.includes(source, Service::Lint)
        {
            return Ok(self.native());
        }

        if self.without_lints.get().is_none() {
            let mut settings = self.settings.clone();
            settings.lint = BTreeMap::from([("*".to_owned(), false)]);

            let configuration =
                instar_bridge::Configuration::new(settings.native_json()?.as_bytes())?;

            drop(self.without_lints.set(configuration));
        }

        Ok(self
            .without_lints
            .get()
            .expect("lint-free configuration initialized"))
    }
}

/// Resolved Roblox settings for a source directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RobloxSettings {
    /// Whether Roblox API types apply.
    pub enabled: bool,

    /// Selected API security level.
    pub security: Security,
}

#[derive(Clone, Default)]
struct Layers {
    legacy: LuauConfig,
    instar: LuauConfig,
    legacy_aliases: BTreeMap<String, Alias>,
    instar_aliases: BTreeMap<String, Alias>,
    inputs: Vec<PathBuf>,
    sourcemaps: Option<Vec<SourcemapLocation>>,
    roblox: RobloxConfig,
    format_options: FormatOptions,
    filters: Filters,
}

/// Cached project snapshot. Entry files may belong to unrelated filesystem roots.
/// Overlay updates require invalidating resolver/checker caches; `analysis::Editor` owns that lifecycle.
/// Create a new snapshot for external filesystem or configuration changes.
#[derive(Default)]
pub struct Project {
    layers: HashMap<PathBuf, Layers>,
    configurations: HashMap<PathBuf, Rc<EffectiveConfig>>,
    sources: HashMap<PathBuf, Rc<str>>,
    overlays: HashMap<PathBuf, Rc<str>>,
    sourcemaps: HashMap<PathBuf, Result<Rc<Sourcemap>, String>>,
    assets: Assets,
}

impl Project {
    /// Creates an empty snapshot; discovery starts from each supplied file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces an unsaved source, or returns it to disk ownership.
    ///
    /// # Errors
    /// Returns an error if the path cannot be normalized.
    pub fn set_source(&mut self, path: &Path, text: Option<&str>) -> io::Result<()> {
        let path = absolute(path)?;
        self.sources.remove(&path);

        if let Some(text) = text {
            self.overlays.insert(path, Rc::from(text));
        } else {
            self.overlays.remove(&path);
        }

        Ok(())
    }

    pub(crate) fn overlay_file(&self, path: &Path) -> bool {
        self.overlays.contains_key(path)
    }

    pub(crate) fn overlay_directory(&self, path: &Path) -> bool {
        self.overlays
            .keys()
            .any(|source| source != path && source.starts_with(path))
    }

    pub(crate) fn overlay_children<'a>(
        &'a self,
        directory: &'a Path,
    ) -> impl Iterator<Item = PathBuf> + 'a {
        self.overlays.keys().filter_map(move |path| {
            path.strip_prefix(directory)
                .ok()?
                .components()
                .next()
                .map(|part| directory.join(part.as_os_str()))
        })
    }

    /// Resolves require expressions without loading declarations or running type analysis.
    ///
    /// # Errors
    /// Returns source or configuration errors.
    pub fn links(&mut self, path: &Path) -> io::Result<Vec<([usize; 2], PathBuf)>> {
        use crate::{
            graph::{self, Request},
            resolve::{Failure, Resolver},
        };

        let source = self.source(path)?;
        let mut resolver = Resolver::new();
        let mut links = Vec::new();

        for module in resolver
            .entries(self, path)
            .map_err(|e| invalid(e.to_string()))?
        {
            let map = module
                .instance
                .as_ref()
                .map(|instance| Rc::clone(&instance.map))
                .map_or_else(|| self.sourcemap(path), |map| Ok(Some(map)))
                .map_err(|e| Failure::Configuration(e.to_string()));

            for site in graph::extract(&source, module.instance.clone(), map).sites {
                let result = match site.request {
                    Some(Request::String(value)) => resolver.resolve(self, &module, &value).result,

                    Some(Request::Instance(instance)) => {
                        Resolver::resolve_instance(&instance).result
                    }

                    None => continue,
                };

                if let Ok(target) = result {
                    links.push((site.range, target.source));
                }
            }
        }

        links.sort();
        links.dedup();

        Ok(links)
    }

    /// Completes a decoded require-string prefix using the regular resolver's navigation rules.
    ///
    /// # Errors
    /// Returns source, configuration, or filesystem errors.
    pub fn complete_import(&mut self, path: &Path, prefix: &str) -> io::Result<Vec<String>> {
        use crate::resolve::{Failure, Resolver};
        let mut resolver = Resolver::new();
        let mut results = std::collections::BTreeSet::new();

        for module in resolver
            .entries(self, path)
            .map_err(|e| invalid(e.to_string()))?
        {
            match resolver.completions(self, &module, prefix) {
                Ok(candidates) => results.extend(candidates),

                Err(Failure::Configuration(error) | Failure::Io(error)) => {
                    return Err(invalid(error));
                }

                Err(_) => {}
            }
        }

        Ok(results.into_iter().collect())
    }

    pub(crate) fn refresh(&mut self) {
        self.layers.clear();
        self.configurations.clear();
        self.sources.clear();
        self.sourcemaps.clear();
        self.assets.invalidate();
    }

    /// Tests global and service selection for a source file, using its inherited configuration.
    /// Excluded files may still be loaded as dependencies.
    ///
    /// # Errors
    /// Returns filesystem, configuration, or invalid-glob errors.
    pub fn includes(&mut self, source: &Path, service: Service) -> io::Result<bool> {
        let source = absolute(source)?;

        Ok(self
            .configuration(&source)?
            .filters
            .includes(&source, service))
    }

    /// Returns inherited format options unless the path is filtered out.
    ///
    /// # Errors
    /// Returns path, configuration, or filter errors.
    pub fn format_options(&mut self, path: &Path) -> io::Result<Option<FormatOptions>> {
        let path = absolute(path)?;
        let config = self.configuration(&path)?;

        if !config.filters.includes(&path, Service::Format) {
            return Ok(None);
        }

        Ok(Some(config.format_options.clone()))
    }

    /// Reads a UTF-8 source once for this snapshot.
    ///
    /// # Errors
    /// Returns filesystem errors or invalid source encoding.
    pub fn source(&mut self, path: &Path) -> io::Result<Rc<str>> {
        let path = absolute(path)?;

        if let Some(source) = self.overlays.get(&path) {
            return Ok(Rc::clone(source));
        }

        if let Some(source) = self.sources.get(&path) {
            return Ok(Rc::clone(source));
        }

        let source: Rc<str> = fs::read_to_string(&path)?.into();
        self.sources.insert(path, Rc::clone(&source));

        Ok(source)
    }

    /// Lists services supplied by the active platform declarations.
    ///
    /// # Errors
    /// Returns configuration or declaration-loading errors.
    pub fn services(&mut self, source: &Path) -> io::Result<Vec<String>> {
        let config = self.configuration(source)?;
        let mut services = std::collections::BTreeSet::new();

        if config.roblox.enabled {
            for location in config.settings.definitions.values() {
                services.extend(
                    self.assets
                        .declaration(location)?
                        .services()
                        .map(str::to_owned),
                );
            }
        }

        Ok(services.into_iter().collect())
    }

    /// Loads JSON documentation for an exact native symbol, with custom entries taking precedence.
    ///
    /// # Errors
    /// Returns configuration, asset-loading, or documentation-validation errors.
    pub fn documentation(
        &mut self,
        source: &Path,
        symbol: &str,
    ) -> io::Result<Option<Rc<serde_json::Value>>> {
        let config = self.configuration(source)?;
        let docs = self.assets.documentation(&config.settings.documentation)?;

        Ok(docs.get(symbol).is_some().then_some(docs))
    }

    /// Drains asset refresh and cache warnings accumulated by this project session.
    pub fn take_asset_warnings(&mut self) -> Vec<String> {
        self.assets.take_warnings()
    }

    pub(crate) fn declarations(
        &mut self,
        locations: &[(String, String)],
    ) -> io::Result<Vec<(String, Rc<Definitions>)>> {
        locations
            .iter()
            .map(|(package, path)| {
                self.assets
                    .declaration(path)
                    .map(|source| (package.clone(), source))
            })
            .collect()
    }

    pub(crate) fn commit_declarations(&mut self, locations: &[(String, String)]) {
        for (_, path) in locations {
            self.assets.commit_declaration(path);
        }
    }

    pub(crate) fn sourcemaps_for(&mut self, source: &Path) -> io::Result<Vec<Rc<Sourcemap>>> {
        let config = self.configuration(source)?;
        let mut maps = Vec::new();

        for location in &config.sourcemaps {
            let result = self
                .sourcemaps
                .entry(location.path.clone())
                .or_insert_with(|| {
                    Sourcemap::load(location.path.clone())
                        .map_err(|error| format!("{}: {error}", location.path.display()))
                });

            match result {
                Ok(map) => {
                    if !maps.iter().any(|existing| Rc::ptr_eq(existing, map)) {
                        maps.push(Rc::clone(map));
                    }
                }

                Err(error) => return Err(invalid(error.clone())),
            }
        }

        Ok(maps)
    }

    pub(crate) fn sourcemap(&mut self, source: &Path) -> io::Result<Option<Rc<Sourcemap>>> {
        let mut maps = self.sourcemaps_for(source)?;

        if maps.len() > 1 {
            return Err(invalid(format!(
                "{} has no unique place context; it is not mapped and multiple sourcemaps are configured",
                source.display()
            )));
        }

        Ok(maps.pop())
    }

    /// Loads effective configuration for a source file's physical directory.
    ///
    /// # Errors
    /// Returns filesystem, configuration syntax, or native validation errors.
    pub fn configuration(&mut self, source: &Path) -> io::Result<Rc<EffectiveConfig>> {
        let source = absolute(source)?;

        self.configuration_at(
            source
                .parent()
                .ok_or_else(|| invalid("source has no parent directory"))?,
        )
    }

    /// Loads inherited configuration at a directory, without a project-root cutoff.
    /// Legacy layers merge ancestor-first, then Instar layers merge ancestor-first.
    ///
    /// # Errors
    /// Returns filesystem, configuration syntax, or native validation errors.
    pub fn configuration_at(&mut self, directory: &Path) -> io::Result<Rc<EffectiveConfig>> {
        let directory = absolute(directory)?;

        if let Some(config) = self.configurations.get(&directory) {
            return Ok(Rc::clone(config));
        }

        let mut layers = self.load_layers(&directory)?;

        let sourcemaps = if layers.roblox.enabled == Some(false) {
            Vec::new()
        } else if let Some(sourcemaps) = layers.sourcemaps {
            sourcemaps
        } else {
            let mut discovered = Vec::new();

            for ancestor in directory.ancestors() {
                let path = ancestor.join("sourcemap.json");
                layers.inputs.push(path.clone());

                match fs::metadata(&path) {
                    Ok(metadata) if metadata.is_file() => {
                        discovered.push(SourcemapLocation {
                            defined_in: path.clone(),
                            path,
                        });

                        break;
                    }

                    Ok(_) => {
                        return Err(invalid(format!(
                            "{}: expected a sourcemap file",
                            path.display()
                        )));
                    }

                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}

                    Err(error) => {
                        return Err(io::Error::new(
                            error.kind(),
                            format!("{}: {error}", path.display()),
                        ));
                    }
                }
            }

            discovered
        };

        let security = layers.roblox.security.unwrap_or_default();

        let roblox = RobloxSettings {
            enabled: layers.roblox.enabled.unwrap_or(!sourcemaps.is_empty()),
            security,
        };

        let mut settings = layers.legacy;
        settings.merge(&layers.instar);

        if roblox.enabled {
            settings
                .definitions
                .entry("@roblox".to_owned())
                .or_insert_with(|| format!("{}{}", assets::BASE, security.file()));

            if settings.documentation.is_empty() {
                settings
                    .documentation
                    .push(format!("{}documentation.json", assets::BASE));
            }
        }

        let mut definition_order: Vec<_> = settings.definitions.keys().cloned().collect();

        if roblox.enabled
            && let Some(index) = definition_order.iter().position(|name| name == "@roblox")
        {
            definition_order[..=index].rotate_right(1);
        }

        let mut aliases = layers.legacy_aliases;
        aliases.extend(layers.instar_aliases);
        let json = settings.native_json()?;
        let native = instar_bridge::Configuration::new(json.as_bytes())?;

        let config = Rc::new(EffectiveConfig {
            settings,
            format_options: layers.format_options,
            aliases,
            json,
            inputs: layers.inputs,
            sourcemaps,
            roblox,
            definition_order,
            native,
            without_lints: OnceCell::new(),
            filters: layers.filters,
        });

        self.configurations.insert(directory, Rc::clone(&config));

        Ok(config)
    }

    fn load_layers(&mut self, directory: &Path) -> io::Result<Layers> {
        if let Some(layers) = self.layers.get(directory) {
            return Ok(layers.clone());
        }

        let mut layers = if let Some(parent) = directory.parent() {
            self.load_layers(parent)?
        } else {
            Layers::default()
        };

        for name in [".luaurc", ".config.luau", "instar.toml"] {
            let path = directory.join(name);
            layers.inputs.push(path.clone());

            let source = match fs::read_to_string(&path) {
                Ok(source) => source,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,

                Err(error) => {
                    return Err(io::Error::new(
                        error.kind(),
                        format!("{}: {error}", path.display()),
                    ));
                }
            };

            let parsed = match name {
                ".luaurc" => config::parse_json(&source),
                ".config.luau" => config::parse_luau(&source),

                _ => toml::from_str::<Config>(&source)
                    .map_err(|e| invalid(e.to_string()))
                    .and_then(|config| {
                        layers.filters.append(directory, &config)?;

                        if let Some(enabled) = config.roblox.enabled {
                            layers.roblox.enabled = Some(enabled);
                        }

                        if let Some(security) = config.roblox.security {
                            layers.roblox.security = Some(security);
                        }

                        if let Some(sourcemaps) = config.roblox.sourcemaps {
                            let mut locations = Vec::new();

                            for sourcemap in sourcemaps {
                                if sourcemap.as_os_str().is_empty()
                                    || sourcemap.as_os_str().as_encoded_bytes().contains(&0)
                                {
                                    return Err(invalid(
                                        "sourcemap path must be nonempty and contain no NUL",
                                    ));
                                }

                                let location = SourcemapLocation {
                                    path: absolute(&directory.join(sourcemap))?,
                                    defined_in: path.clone(),
                                };

                                if !locations.contains(&location) {
                                    locations.push(location);
                                }
                            }

                            layers.sourcemaps = Some(locations);
                        }

                        layers.format_options.merge(&config.format)?;

                        Ok(config.luau)
                    }),
            };

            let mut config = parsed.map_err(|e| invalid(format!("{}: {e}", path.display())))?;

            config
                .validate()
                .map_err(|e| invalid(format!("{}: {e}", path.display())))?;

            for location in config
                .definitions
                .values_mut()
                .chain(config.documentation.iter_mut())
            {
                *location = assets::location(location, directory)
                    .map_err(|error| invalid(format!("{}: {error}", path.display())))?;
            }

            let (settings, aliases) = if name == "instar.toml" {
                (&mut layers.instar, &mut layers.instar_aliases)
            } else {
                (&mut layers.legacy, &mut layers.legacy_aliases)
            };

            settings.merge(&config);

            for (name, target) in config.aliases {
                aliases.insert(
                    name,
                    Alias {
                        target,
                        defined_in: path.clone(),
                    },
                );
            }
        }

        self.layers.insert(directory.to_owned(), layers.clone());

        Ok(layers)
    }
}
