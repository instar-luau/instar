//! Configuration discovery and independent Instar and Luau inheritance.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

use crate::{
    configuration::{Configuration, invalid},
    source::{Document, Failure, Kind, View, normalize, related},
    sourcemap::Map,
};

/// Resolved configuration for a source directory.
pub struct Settings {
    /// Effective Instar settings with absolute file references and anchored patterns.
    pub configuration: Configuration,

    /// Independently inherited native Luau settings.
    pub native: instar_bridge::Configuration,

    /// Configuration files applied in ancestor-to-descendant order.
    pub files: Vec<PathBuf>,

    /// Present and absent configuration and discovery candidates.
    pub inputs: BTreeSet<PathBuf>,

    /// Detached effective upstream settings.
    pub snapshot: instar_bridge::Snapshot,
}

impl Settings {
    fn load(view: &mut View, directory: &Path, timeout: Duration) -> io::Result<Self> {
        if view.kind(directory)? != Some(Kind::Directory) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration discovery requires a directory",
            ));
        }

        let mut ancestors = directory.ancestors().collect::<Vec<_>>();
        ancestors.reverse();
        let mut merged = toml::Value::Table(toml::map::Map::new());
        let mut native = instar_bridge::Configuration::new()?;
        let mut files = Vec::new();
        let mut discovered_map = None;

        for ancestor in ancestors {
            let manifest = ancestor.join("instar.toml");

            if let Some(source) = view.read(&manifest)? {
                Configuration::parse(&source.text).map_err(|error| located(&manifest, error))?;

                let mut layer = toml::from_str::<toml::Value>(&source.text)
                    .map_err(|error| located(&manifest, error))?;

                anchor(&mut layer, ancestor).map_err(|error| located(&manifest, error))?;
                merge(&mut merged, layer);
                files.push(manifest);
            }

            let json = ancestor.join(".luaurc");
            let luau = ancestor.join(".config.luau");
            let json_source = view.read(&json)?;
            let luau_source = view.read(&luau)?;

            match (json_source, luau_source) {
                (Some(_), Some(_)) => {
                    return Err(invalid(format!(
                        "{}: .luaurc and .config.luau cannot coexist in one directory",
                        ancestor.display()
                    )));
                }

                (Some(source), None) => {
                    native.apply(&source.text, &json, timeout)?;
                    files.push(json);
                }

                (None, Some(source)) => {
                    native.apply(&source.text, &luau, timeout)?;
                    files.push(luau);
                }

                (None, None) => {}
            }

            let map = ancestor.join("sourcemap.json");

            if view.kind(&map)? == Some(Kind::File) {
                discovered_map = Some(map);
            }
        }

        let mut configuration: Configuration = merged.try_into().map_err(invalid)?;
        configuration.inherit_selection();

        if configuration.roblox.sourcemaps.is_none() {
            configuration.roblox.sourcemaps = Some(discovered_map.into_iter().collect());
        }

        if configuration.roblox.enabled.is_none() {
            configuration.roblox.enabled = Some(
                configuration
                    .roblox
                    .sourcemaps
                    .as_ref()
                    .is_some_and(|maps| !maps.is_empty()),
            );
        }

        configuration.validate()?;

        Ok(Self {
            configuration,
            snapshot: native.snapshot()?,
            native,
            files,
            inputs: view.consulted.clone(),
        })
    }
}

/// A host-supplied source or filesystem lifecycle event.
#[derive(Clone, Debug)]
pub enum Change {
    /// Disk contents or metadata changed, including create, delete and replacement.
    Disk(PathBuf),

    /// Replace an editor overlay; `None` masks a deleted file.
    Overlay {
        /// Absolute source or configuration path.
        path: PathBuf,
        /// New contents, or an overlay deletion.
        text: Option<String>,
    },

    /// Stop overriding a path and read its disk contents again.
    Close(PathBuf),
}

struct CachedSettings {
    result: Result<Rc<Settings>, Failure>,
    inputs: BTreeSet<PathBuf>,
}

/// Cached project sources, effective settings and module graph.
pub struct Project {
    pub(crate) view: View,
    timeout: Duration,
    settings: BTreeMap<PathBuf, CachedSettings>,
    maps: BTreeMap<PathBuf, Result<Rc<Map>, Failure>>,
    pub(crate) graph: crate::graph::Graph,
    pub(crate) frontend: Option<instar_bridge::frontend::Frontend>,
}

impl Project {
    /// Creates an empty project view with a native configuration execution timeout.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            view: View::default(),
            timeout,
            settings: BTreeMap::new(),
            maps: BTreeMap::new(),
            graph: crate::graph::Graph::default(),
            frontend: None,
        }
    }

    /// Reads an immutable source snapshot, honoring editor overlays.
    ///
    /// # Errors
    /// Returns source loading errors or rejects relative paths.
    pub fn source(&mut self, path: &Path) -> io::Result<Document> {
        let path = absolute(path)?;

        self.view.read(&path)?.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("source not found: {}", path.display()),
            )
        })
    }

    /// Returns cached settings for an absolute source directory.
    ///
    /// # Errors
    /// Returns configuration discovery, parsing or native execution errors.
    pub fn configuration(&mut self, directory: &Path) -> io::Result<Rc<Settings>> {
        let directory = absolute(directory)?;

        if let Some(settings) = self.settings.get(&directory) {
            self.view.consulted.extend(settings.inputs.iter().cloned());

            return settings.result.clone().map_err(|error| error.error());
        }

        let outer = std::mem::take(&mut self.view.consulted);

        let result = Settings::load(&mut self.view, &directory, self.timeout)
            .map(Rc::new)
            .map_err(Failure::from);

        let inputs = std::mem::take(&mut self.view.consulted);
        self.view.consulted = outer;
        self.view.consulted.extend(inputs.iter().cloned());

        self.settings.insert(
            directory,
            CachedSettings {
                result: result.clone(),
                inputs,
            },
        );

        result.map_err(|error| error.error())
    }

    pub(crate) fn map(&mut self, path: &Path) -> io::Result<Rc<Map>> {
        let path = absolute(path)?;
        self.view.consulted.insert(path.clone());

        if let Some(result) = self.maps.get(&path) {
            return result.clone().map_err(|error| error.error());
        }

        let result = self
            .source(&path)
            .and_then(|document| Map::parse(&path, &document))
            .map_err(Failure::from);

        self.maps.insert(path, result.clone());

        result.map_err(|error| error.error())
    }

    pub(crate) fn maps(&mut self, directory: &Path) -> io::Result<Vec<Rc<Map>>> {
        let settings = self.configuration(directory)?;

        if settings.configuration.roblox.enabled != Some(true) {
            return Ok(Vec::new());
        }

        let mut maps = Vec::new();

        for path in settings.configuration.roblox.sourcemaps.iter().flatten() {
            if !maps.iter().any(|map: &Rc<Map>| &map.path == path) {
                maps.push(self.map(path)?);
            }
        }

        Ok(maps)
    }

    /// Applies a host event, evicting affected resolutions and reverse dependents.
    ///
    /// # Errors
    /// Rejects relative event paths.
    pub fn change(&mut self, change: Change) -> io::Result<BTreeSet<crate::resolve::Identity>> {
        let path = match &change {
            Change::Disk(path) | Change::Close(path) | Change::Overlay { path, .. } => path,
        };

        let path = absolute(path)?;
        let changed = self.view.changes(&path);
        self.view.change(&path, change);

        self.settings.retain(|_, settings| {
            !settings
                .inputs
                .iter()
                .any(|input| changed.iter().any(|path| related(input, path)))
        });

        self.maps
            .retain(|candidate, _| !changed.iter().any(|path| related(candidate, path)));

        let mut affected = BTreeSet::new();

        for path in changed {
            affected.extend(self.graph.invalidate(&path));
        }

        if let Some(frontend) = &mut self.frontend {
            frontend.invalidate(&affected.iter().map(crate::native::name).collect::<Vec<_>>())?;
        }

        Ok(affected)
    }

    /// Returns every consulted present and absent host watch input.
    #[must_use]
    pub fn watch_inputs(&self) -> BTreeSet<PathBuf> {
        self.view.consulted.clone()
    }
}

pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("path must be absolute"));
    }

    Ok(normalize(path))
}

fn located(path: &Path, error: impl std::fmt::Display) -> io::Error {
    invalid(format!("{}: {error}", path.display()))
}

fn merge(parent: &mut toml::Value, child: toml::Value) {
    match (parent, child) {
        (toml::Value::Table(parent), toml::Value::Table(child)) => {
            for (name, value) in child {
                match parent.get_mut(&name) {
                    Some(inherited) => merge(inherited, value),

                    None => {
                        parent.insert(name, value);
                    }
                }
            }
        }

        (parent, child) => *parent = child,
    }
}

fn anchor(layer: &mut toml::Value, directory: &Path) -> io::Result<()> {
    for section in [
        &[][..],
        &["check"],
        &["format"],
        &["lint"],
        &["editor", "index"],
        &["editor", "imports"],
    ] {
        for field in ["include", "exclude"] {
            anchor_list(layer, section, field, directory, true)?;
        }
    }

    for (section, field) in [
        (&["environment"][..], "definitions"),
        (&["environment"][..], "documentation"),
        (&["roblox"][..], "sourcemaps"),
    ] {
        anchor_list(layer, section, field, directory, false)?;
    }

    Ok(())
}

fn anchor_list(
    layer: &mut toml::Value,
    section: &[&str],
    field: &str,
    directory: &Path,
    pattern: bool,
) -> io::Result<()> {
    let mut value = layer;

    for name in section {
        let Some(child) = value.get_mut(*name) else {
            return Ok(());
        };

        value = child;
    }

    let Some(toml::Value::Array(values)) = value.get_mut(field) else {
        return Ok(());
    };

    for value in values {
        let toml::Value::String(text) = value else {
            return Err(invalid("file references must be strings"));
        };

        if Path::new(text).is_absolute() {
            continue;
        }

        let directory = directory
            .to_str()
            .ok_or_else(|| invalid("configuration directories must be UTF-8"))?;

        let prefix = if pattern {
            glob::Pattern::escape(&directory.replace('\\', "/"))
        } else {
            directory.replace('\\', "/")
        };

        *text = format!("{prefix}/{text}");
    }

    Ok(())
}
