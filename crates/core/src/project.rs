//! Configuration discovery and independent Instar and Luau inheritance.

use std::{
    collections::{BTreeMap, BTreeSet},
    io, mem,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

use instar_analysis::{Options, error::invalid, selection};
use instar_bridge::{self as bridge, Snapshot, flags, frontend::Frontend};
use toml::{Value, map};

use crate::{
    configuration::Configuration,
    graph::Graph,
    native,
    resolve::Identity,
    roblox::Assets,
    snapshot,
    source::{Document, Failure, Kind, View, normalize, related},
    sourcemap::Map,
};

/// Resolved configuration for a source directory.
pub struct Settings {
    /// Effective Instar settings with absolute file references and anchored patterns.
    pub configuration: Configuration,

    /// Independently inherited native Luau settings.
    pub native: bridge::Configuration,

    /// Configuration files applied in ancestor-to-descendant order.
    pub files: Vec<PathBuf>,

    /// Present and absent configuration and discovery candidates.
    pub inputs: BTreeSet<PathBuf>,

    /// Detached effective upstream settings.
    pub snapshot: Snapshot,

    /// Validated effective native flag overrides.
    pub flags: flags::Overrides,
}

impl Settings {
    fn load(
        directory: &Path,
        view: &mut View,
        timeout: Duration,
        operation: Option<&(Options, Instant)>,
    ) -> io::Result<Self> {
        if view.kind(directory)? != Some(Kind::Directory) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "configuration discovery requires a directory",
            ));
        }

        let mut ancestors = directory.ancestors().collect::<Vec<_>>();
        ancestors.reverse();
        let mut merged = Value::Table(map::Map::new());
        let mut native = bridge::Configuration::new()?;
        let mut files = Vec::new();
        let mut discovered_map = None;

        for ancestor in ancestors {
            if let Some((options, started)) = operation {
                options.check(*started)?;
            }

            let manifest = ancestor.join("instar.toml");

            if let Some(source) = view.read(&manifest)? {
                Configuration::parse(&source.text).map_err(|error| located(&manifest, &error))?;

                let mut layer = toml::from_str::<Value>(&source.text)
                    .map_err(|error| located(&manifest, &invalid(error)))?;

                anchor(ancestor, &mut layer).map_err(|error| located(&manifest, &error))?;
                merge(layer, &mut merged);
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
                    let mut remaining = operation.map_or_else(
                        || Options::new(timeout),
                        |(options, started)| options.remaining(*started),
                    );

                    remaining.timeout = remaining.timeout.min(timeout);

                    native.apply(&source.text, &json, &remaining)?;
                    files.push(json);
                }

                (None, Some(source)) => {
                    let mut remaining = operation.map_or_else(
                        || Options::new(timeout),
                        |(options, started)| options.remaining(*started),
                    );

                    remaining.timeout = remaining.timeout.min(timeout);

                    native.apply(&source.text, &luau, &remaining)?;
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

        let flags = configuration.validate()?;

        Ok(Self {
            configuration,
            flags,
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
    pub(crate) timeout: Duration,
    pub(crate) operation: Option<(Options, Instant)>,
    settings: BTreeMap<PathBuf, CachedSettings>,
    maps: BTreeMap<PathBuf, Result<Rc<Map>, Failure>>,
    pub(crate) graph: Graph,
    pub(crate) frontend: Option<Frontend>,
    pub(crate) roblox: BTreeMap<[u64; 4], Rc<Assets>>,
}

impl Project {
    /// Creates an empty project view with a native configuration execution timeout.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            view: View::default(),
            timeout,
            operation: None,
            settings: BTreeMap::new(),
            maps: BTreeMap::new(),
            graph: Graph::default(),
            frontend: None,
            roblox: BTreeMap::new(),
        }
    }

    /// Reads an immutable source snapshot, honoring editor overlays.
    ///
    /// # Errors
    /// Returns source loading errors or rejects relative paths.
    pub fn source(&mut self, path: &Path) -> io::Result<Document> {
        self.interrupted()?;
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
        self.interrupted()?;
        let directory = absolute(directory)?;

        if let Some(settings) = self.settings.get(&directory) {
            self.view.consult(&settings.inputs);

            return settings.result.clone().map_err(|error| error.error());
        }

        let outer = mem::take(&mut self.view.consulted);

        let result = Settings::load(
            &directory,
            &mut self.view,
            self.timeout,
            self.operation.as_ref(),
        )
        .map(Rc::new)
        .map_err(Failure::from);

        if let Err(error) = self.interrupted() {
            let inputs = mem::take(&mut self.view.consulted);
            self.view.consulted = outer;
            self.view.consult(&inputs);

            return Err(error);
        }

        let inputs = mem::take(&mut self.view.consulted);
        self.view.consulted = outer;
        self.view.consult(&inputs);

        self.settings.insert(
            directory,
            CachedSettings {
                result: result.clone(),
                inputs,
            },
        );

        result.map_err(|error| error.error())
    }

    /// Applies a host event, evicting affected resolutions and reverse dependents.
    ///
    /// # Errors
    /// Rejects relative event paths.
    pub fn change(&mut self, change: Change) -> io::Result<BTreeSet<Identity>> {
        let path = match &change {
            Change::Disk(path) | Change::Close(path) | Change::Overlay { path, .. } => {
                absolute(path)?
            }
        };

        self.view.snapshot.change(change)?;

        Ok(self.invalidate(&path))
    }

    /// Adopts host inputs, retaining caches unaffected by their changes.
    pub fn apply(&mut self, snapshot: &snapshot::Snapshot) {
        for path in snapshot.changes(&self.view.snapshot) {
            self.invalidate(&path);
        }

        self.view.snapshot = snapshot.clone();
    }

    /// Returns every consulted present and absent host watch input.
    #[must_use]
    pub fn watch_inputs(&self) -> BTreeSet<PathBuf> {
        self.view.consulted.clone()
    }

    pub(crate) fn during<Output>(
        &mut self,
        action: impl FnOnce(&mut Self) -> io::Result<Output>,
        options: &Options,
    ) -> io::Result<Output> {
        let started = Instant::now();
        options.check(started)?;
        let previous = self.operation.replace((options.clone(), started));
        let result = action(self);
        self.operation = previous;
        options.check(started)?;

        result
    }

    pub(crate) fn interrupted(&self) -> io::Result<()> {
        if let Some((options, started)) = &self.operation {
            options.check(*started)?;
        }

        Ok(())
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

    fn invalidate(&mut self, path: &Path) -> BTreeSet<Identity> {
        let changed = self.view.changes(path);
        self.view.invalidate(path);

        self.roblox.retain(|_, assets| {
            !assets.definitions.iter().any(|(source, _)| {
                source.parent().is_some_and(|directory| {
                    path.starts_with(directory) || directory.starts_with(path)
                })
            })
        });

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
            frontend.invalidate(&affected.iter().map(native::name).collect::<Vec<_>>());
        }

        affected
    }
}

pub(crate) fn absolute(path: &Path) -> io::Result<PathBuf> {
    if !path.is_absolute() {
        return Err(invalid("path must be absolute"));
    }

    Ok(normalize(path))
}

fn located(path: &Path, error: &io::Error) -> io::Error {
    io::Error::new(error.kind(), format!("{}: {error}", path.display()))
}

fn merge(child: Value, parent: &mut Value) {
    match (parent, child) {
        (Value::Table(parent), Value::Table(child)) => {
            for (name, value) in child {
                match parent.get_mut(&name) {
                    Some(inherited) => merge(value, inherited),

                    None => {
                        parent.insert(name, value);
                    }
                }
            }
        }

        (parent, child) => *parent = child,
    }
}

fn anchor(directory: &Path, layer: &mut Value) -> io::Result<()> {
    for section in [
        &[][..],
        &["check"],
        &["format"],
        &["lint"],
        &["editor", "index"],
        &["editor", "imports"],
    ] {
        for field in ["include", "exclude"] {
            anchor_list(section, field, directory, layer, true)?;
        }
    }

    anchor_list(&["roblox"], "sourcemaps", directory, layer, false)?;

    if let Some(Value::Array(environments)) = layer.get_mut("environment") {
        for environment in environments {
            for field in ["definitions", "documentation"] {
                anchor_list(&[], field, directory, environment, false)?;
            }
        }
    }

    Ok(())
}

fn anchor_list(
    section: &[&str],
    field: &str,
    directory: &Path,
    layer: &mut Value,
    pattern: bool,
) -> io::Result<()> {
    let mut value = layer;

    for name in section {
        let Some(child) = value.get_mut(*name) else {
            return Ok(());
        };

        value = child;
    }

    let Some(Value::Array(values)) = value.get_mut(field) else {
        return Ok(());
    };

    for value in values {
        let Value::String(text) = value else {
            return Err(invalid("file references must be strings"));
        };

        if !pattern {
            let path = normalize(&directory.join(&*text));

            *text = path
                .to_str()
                .ok_or_else(|| invalid("configuration paths must be UTF-8"))?
                .replace('\\', "/");

            continue;
        }

        *text = selection::anchor(directory, text)?;
    }

    Ok(())
}
