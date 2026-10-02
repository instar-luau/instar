//! Project configuration discovery and source snapshots.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Mutex,
};

use crate::{
    absolute,
    assets::{self, Assets, Definitions},
    config::{
        self, Config, FlagValue, FormatOptions, ImportsConfig, LintConfig, LuauConfig,
        LuauFlagsConfig, RobloxConfig, Security,
    },
    filter::{Filters, Service},
    invalid,
    roblox::{Sourcemap, SourcemapLocation},
};

static INSTALLED_FLAGS: Mutex<Option<LuauFlagsConfig>> = Mutex::new(None);

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
    /// Merged settings, with manifest values taking precedence over legacy formats.
    pub settings: LuauConfig,

    /// Effective formatting settings for the source directory.
    pub format_options: FormatOptions,

    /// Merged lint settings for this source directory.
    pub lint: LintConfig,

    /// Merged autoimport preferences and selection for this source directory.
    pub imports: ImportsConfig,

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
    filters: Filters,
}

impl EffectiveConfig {
    /// Borrows the native configuration built from the merged JSON.
    #[must_use]
    pub const fn native(&self) -> &instar_bridge::Configuration {
        &self.native
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
    manifest: LuauConfig,
    legacy_aliases: BTreeMap<String, Alias>,
    manifest_aliases: BTreeMap<String, Alias>,
    inputs: Vec<PathBuf>,
    sourcemaps: Option<Vec<SourcemapLocation>>,
    roblox: RobloxConfig,
    format_options: FormatOptions,
    lint: LintConfig,
    imports: ImportsConfig,
    filters: Filters,
}

/// Cached project snapshot. Entry files may belong to unrelated filesystem roots.
/// Overlay updates require invalidating resolver/checker caches; [`crate::editor::Editor`] owns that lifecycle.
/// Create a new snapshot for external filesystem or configuration changes.
#[derive(Default)]
pub struct Project {
    layers: HashMap<PathBuf, Layers>,
    configurations: HashMap<PathBuf, Rc<EffectiveConfig>>,
    sources: HashMap<PathBuf, Rc<str>>,
    overlays: HashMap<PathBuf, Rc<str>>,
    definitions: HashMap<String, Rc<Definitions>>,
    sourcemaps: HashMap<PathBuf, Result<Rc<Sourcemap>, String>>,
    assets: Assets,
}

impl Project {
    /// Creates an empty snapshot; discovery starts from each supplied file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies the one process-wide Luau flag snapshot before semantic preparation begins.
    ///
    /// # Errors
    /// Returns configuration, cache, or native flag errors; conflicting projects require a restart.
    pub fn prepare_fast_flags(&mut self, paths: &[PathBuf]) -> io::Result<bool> {
        if paths.is_empty() {
            return Ok(false);
        }

        let mut selected: Option<LuauFlagsConfig> = None;

        for path in paths {
            let directory = if path.is_dir() {
                path.as_path()
            } else {
                path.parent()
                    .ok_or_else(|| invalid("source path has no parent directory"))?
            };

            let config = self.configuration_at(directory)?;

            let mut flags = config.settings.fflags.clone();
            flags.sync_roblox = Some(flags.sync_roblox.unwrap_or(false));

            if selected.as_ref().is_some_and(|previous| previous != &flags) {
                return Err(invalid(
                    "conflicting Luau FFlag settings across project roots",
                ));
            }

            selected = Some(flags);
        }

        let selected = selected.unwrap_or_default();
        let sync = selected.sync_roblox == Some(true);
        let enabled = sync || !selected.overrides.is_empty();

        let mut installed = INSTALLED_FLAGS
            .lock()
            .map_err(|_| invalid("Luau flag state was poisoned"))?;

        if let Some(previous) = installed.as_ref() {
            if previous != &selected {
                return Err(invalid("Luau FFlag changes require restarting the process"));
            }

            return Ok(sync);
        }

        let registry = if enabled {
            instar_bridge::fast_flags()?
        } else {
            BTreeMap::new()
        };

        let mut flags = if sync {
            self.assets.studio_flags(&registry)
        } else {
            BTreeMap::new()
        };

        for (name, value) in &selected.overrides {
            let value = match (value, registry.get(name)) {
                (FlagValue::Bool(value), Some(instar_bridge::FastFlagValue::Bool(_))) => {
                    instar_bridge::FastFlagValue::Bool(*value)
                }

                (FlagValue::Int(value), Some(instar_bridge::FastFlagValue::Int(_))) => {
                    instar_bridge::FastFlagValue::Int(*value)
                }

                (_, None) => return Err(invalid(format!("unknown Luau fast flag {name:?}"))),

                _ => {
                    return Err(invalid(format!(
                        "wrong value type for Luau fast flag {name:?}"
                    )));
                }
            };

            flags.insert(name.clone(), value);
        }

        for (name, value) in &flags {
            instar_bridge::set_fast_flag(name, *value)?;
        }

        *installed = Some(selected);

        Ok(sync)
    }

    /// Refreshes the cached Studio flag snapshot without changing active native flags.
    ///
    /// # Errors
    /// Returns cache or native registry errors.
    pub fn refresh_fast_flag_cache() -> io::Result<Vec<String>> {
        let mut assets = Assets::default();
        assets.studio_flags(&instar_bridge::fast_flags()?);

        Ok(assets.take_warnings())
    }

    /// Replaces an unsaved source, or returns it to disk ownership.
    ///
    /// # Errors
    /// Returns an error if the path cannot be normalized.
    pub fn set_source(&mut self, path: &Path, text: Option<&str>) -> io::Result<()> {
        let path = absolute(path)?;
        self.sources.remove(&path);

        if let Some(location) = path.to_str() {
            self.definitions.remove(location);
        }

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
        self.definitions.clear();
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

    /// Tests a candidate against the caller's and target's global and autoimport selection.
    /// Caller exclusions are checked before loading the target's configuration.
    ///
    /// # Errors
    /// Returns filesystem, configuration, or invalid-glob errors.
    pub fn includes_import(&mut self, source: &Path, target: &Path) -> io::Result<bool> {
        let target = absolute(target)?;

        if !self
            .configuration(source)?
            .filters
            .includes(&target, Service::Imports)
        {
            return Ok(false);
        }

        self.includes(&target, Service::Imports)
    }

    /// Prunes a directory only when an inherited literal `directory/**` rule excludes its subtree.
    ///
    /// # Errors
    /// Returns path or configuration errors.
    pub fn excludes_subtree(&mut self, directory: &Path, service: Service) -> io::Result<bool> {
        let directory = absolute(directory)?;
        let parent = directory.parent().unwrap_or(&directory);

        Ok(self
            .load_layers(parent)?
            .filters
            .excludes_subtree(&directory, service))
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
        self.source_with(path, |path| fs::read_to_string(path))
    }

    fn source_with(
        &mut self,
        path: &Path,
        load: impl FnOnce(&Path) -> io::Result<String>,
    ) -> io::Result<Rc<str>> {
        let path = absolute(path)?;

        if let Some(source) = self.overlays.get(&path) {
            return Ok(Rc::clone(source));
        }

        if let Some(source) = self.sources.get(&path) {
            return Ok(Rc::clone(source));
        }

        let source: Rc<str> = load(&path)?.into();
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
                services.extend(self.declaration(location)?.services().map(str::to_owned));
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

    fn declaration(&mut self, location: &str) -> io::Result<Rc<Definitions>> {
        let source = if location.starts_with("https://") {
            self.assets.declaration(location)?
        } else {
            self.source_with(Path::new(location), assets::read_declaration)?
        };

        if let Some(definition) = self.definitions.get(location)
            && Rc::ptr_eq(&source, &definition.source)
        {
            return Ok(Rc::clone(definition));
        }

        let definition = Rc::new(
            Definitions::new(source).map_err(|error| invalid(format!("{location}: {error}")))?,
        );

        self.definitions
            .insert(location.to_owned(), Rc::clone(&definition));

        Ok(definition)
    }

    pub(crate) fn declarations(
        &mut self,
        locations: &[(String, String)],
    ) -> io::Result<Vec<(String, Rc<Definitions>)>> {
        locations
            .iter()
            .map(|(package, path)| {
                self.declaration(path)
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
    /// Legacy format layers merge ancestor-first, then manifest layers merge ancestor-first.
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
        settings.merge(&layers.manifest);

        if let Some(lint_errors) = layers.lint.lint_errors {
            settings.lint_errors = Some(lint_errors);
        }

        if layers.lint.luau.contains_key("*") {
            settings.lint.clear();
        }

        settings.lint.extend(layers.lint.luau.clone());

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
        aliases.extend(layers.manifest_aliases);
        let json = settings.native_json()?;
        let native = instar_bridge::Configuration::new(json.as_bytes())?;

        let config = Rc::new(EffectiveConfig {
            settings,
            format_options: layers.format_options,
            lint: layers.lint,
            imports: layers.imports,
            aliases,
            json,
            inputs: layers.inputs,
            sourcemaps,
            roblox,
            definition_order,
            native,
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
                        config.lint.validate()?;
                        layers.lint.merge(&config.lint);
                        layers.imports.merge(&config.lsp.imports);
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

                        Ok(config.luau.into())
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
                (&mut layers.manifest, &mut layers.manifest_aliases)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BindingStyle, RequireStyle};

    #[test]
    fn imports_inherit_preferences_and_check_both_scopes_before_loading_excluded_targets() {
        let root = std::env::temp_dir().join(format!("import-selection-{}", std::process::id()));
        let caller = root.join("place/nested");
        let modules = root.join("modules");
        let blocked = root.join("blocked");
        fs::create_dir_all(&caller).unwrap();
        fs::create_dir_all(&modules).unwrap();
        fs::create_dir_all(&blocked).unwrap();

        fs::write(
            root.join("instar.toml"),
            r#"
exclude = ["blocked/**"]
[roblox]
enabled = false
[lsp]
exclude = ["modules/**"]
[lsp.imports]
require = "string"
binding = "const"
"#,
        )
        .unwrap();

        fs::write(
            root.join("place/instar.toml"),
            "[lsp.imports]\nrequire = \"instance\"\nexclude = [\"../modules/caller-skip.luau\"]",
        )
        .unwrap();

        fs::write(caller.join("instar.toml"), "[lsp.imports]\ninclude = []").unwrap();

        fs::write(
            modules.join("instar.toml"),
            "exclude = [\"global-skip.luau\"]\n[lsp.imports]\nexclude = [\"target-skip.luau\"]",
        )
        .unwrap();

        fs::write(
            blocked.join("instar.toml"),
            "[lsp.imports]\nrequire = \"invalid\"",
        )
        .unwrap();

        let source = caller.join("main.luau");
        let mut project = Project::new();
        let configuration = project.configuration(&source).unwrap();
        assert_eq!(configuration.imports.require, Some(RequireStyle::Instance));
        assert_eq!(configuration.imports.binding, Some(BindingStyle::Const));

        assert_eq!(
            configuration.imports.exclude,
            ["../modules/caller-skip.luau"]
        );

        assert!(
            project
                .includes(&modules.join("public.luau"), Service::Imports)
                .unwrap()
        );

        assert!(
            !project
                .includes(&modules.join("public.luau"), Service::Lsp)
                .unwrap()
        );

        for (target, included) in [
            ("modules/public.luau", true),
            ("modules/caller-skip.luau", false),
            ("modules/target-skip.luau", false),
            ("modules/global-skip.luau", false),
            ("blocked/init.luau", false),
        ] {
            assert_eq!(
                project
                    .includes_import(&source, &root.join(target))
                    .unwrap(),
                included,
                "{target}"
            );
        }

        assert!(project.configuration(&blocked.join("init.luau")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
