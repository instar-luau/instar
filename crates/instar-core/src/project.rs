//! Project configuration discovery and source snapshots.

use std::{
    collections::{BTreeMap, HashMap},
    fs, io,
    path::{Path, PathBuf},
    rc::Rc,
};

use crate::{
    absolute,
    config::{self, Config, LuauConfig},
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

    /// Alias definitions, indexed by lowercase name.
    pub aliases: BTreeMap<String, Alias>,

    /// Native-compatible JSON representation of the effective settings.
    pub json: String,

    /// Configuration lookup dependencies, including files that did not exist.
    pub inputs: Vec<PathBuf>,

    /// Effective sourcemaps, retaining their explicit or automatically discovered origin.
    pub sourcemaps: Vec<SourcemapLocation>,

    native: instar_bridge::Configuration,
}

impl EffectiveConfig {
    /// Borrows the native configuration built from the merged JSON.
    #[must_use]
    pub const fn native(&self) -> &instar_bridge::Configuration {
        &self.native
    }
}

#[derive(Clone, Default)]
struct Layers {
    legacy: LuauConfig,
    instar: LuauConfig,
    legacy_aliases: BTreeMap<String, Alias>,
    instar_aliases: BTreeMap<String, Alias>,
    inputs: Vec<PathBuf>,
    sourcemaps: Option<Vec<SourcemapLocation>>,
}

/// Cached project snapshot. Entry files may belong to unrelated filesystem roots.
/// Create a new snapshot after source, configuration, or filesystem changes.
#[derive(Default)]
pub struct Project {
    layers: HashMap<PathBuf, Layers>,
    configurations: HashMap<PathBuf, Rc<EffectiveConfig>>,
    sources: HashMap<PathBuf, Rc<str>>,
    sourcemaps: HashMap<PathBuf, Result<Rc<Sourcemap>, String>>,
}

impl Project {
    /// Creates an empty snapshot; discovery starts from each supplied file.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads a UTF-8 source once for this snapshot.
    ///
    /// # Errors
    /// Returns filesystem errors or invalid source encoding.
    pub fn source(&mut self, path: &Path) -> io::Result<Rc<str>> {
        let path = absolute(path)?;

        if let Some(source) = self.sources.get(&path) {
            return Ok(Rc::clone(source));
        }

        let source: Rc<str> = fs::read_to_string(&path)?.into();
        self.sources.insert(path, Rc::clone(&source));

        Ok(source)
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

        let sourcemaps = if let Some(sourcemaps) = layers.sourcemaps {
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

        let mut settings = layers.legacy;
        settings.merge(&layers.instar);
        let mut aliases = layers.legacy_aliases;
        aliases.extend(layers.instar_aliases);
        let json = settings.native_json()?;
        let native = instar_bridge::Configuration::new(json.as_bytes())?;

        let config = Rc::new(EffectiveConfig {
            settings,
            aliases,
            json,
            inputs: layers.inputs,
            sourcemaps,
            native,
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

                        Ok(config.luau)
                    }),
            };

            let mut config = parsed.map_err(|e| invalid(format!("{}: {e}", path.display())))?;

            config
                .validate()
                .map_err(|e| invalid(format!("{}: {e}", path.display())))?;

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
