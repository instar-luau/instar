use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    graft::Mapping,
    source::{PositionEncoding, Source, SourceStore, absolute},
};

use line_index::LineCol;

struct Node {
    is_directory: bool,
    module: Option<PathBuf>,
}

struct Lowering {
    original: Arc<Source>,
    generated: Arc<Source>,
    mappings: Vec<Mapping>,
}

/// Module resolution using consistent source and configuration snapshots.
pub struct Resolver<'store> {
    sources: &'store mut SourceStore,
    snapshots: BTreeMap<PathBuf, Arc<Source>>,
    lowerings: BTreeMap<PathBuf, Lowering>,
    configurations: BTreeMap<PathBuf, super::Configuration>,
    pub(crate) discovery: super::discovery::Discovery,
}

impl<'store> Resolver<'store> {
    /// Create a resolver backed by the supplied source store.
    pub fn new(sources: &'store mut SourceStore) -> Self {
        Self {
            sources,
            snapshots: BTreeMap::new(),
            lowerings: BTreeMap::new(),
            configurations: BTreeMap::new(),
            discovery: super::discovery::Discovery::default(),
        }
    }

    fn configuration(&mut self, path: &Path) -> io::Result<&super::Configuration> {
        let directory = path
            .parent()
            .ok_or_else(|| io::Error::other("source has no parent"))?
            .to_owned();

        if !self.configurations.contains_key(&directory) {
            let configuration = super::Configuration::discover_frontends(path)?;
            self.configurations.insert(directory.clone(), configuration);
        }

        Ok(self
            .configurations
            .get(&directory)
            .expect("configuration inserted"))
    }

    /// # Errors
    /// Returns path and source-loading failures.
    pub fn load(&mut self, path: &Path) -> io::Result<Arc<Source>> {
        let path = absolute(path).map_err(io::Error::other)?;

        if let Some(source) = self.snapshots.get(&path) {
            return Ok(Arc::clone(source));
        }

        let original = self.sources.read(&path).map_err(io::Error::other)?;

        let compilation = if matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("lua" | "luau") | None
        ) {
            None
        } else {
            self.configuration(&path)?
                .frontend(&path)
                .map(|graft| graft.compile(original.bytes()))
                .transpose()?
        };

        let source = if let Some(compilation) = compilation {
            let generated = Arc::new(
                Source::new(path.clone(), compilation.source.into_bytes())
                    .map_err(io::Error::other)?,
            );

            self.lowerings.insert(
                path.clone(),
                Lowering {
                    original,
                    generated: Arc::clone(&generated),
                    mappings: compilation.mappings,
                },
            );

            generated
        } else {
            original
        };

        self.snapshots.insert(path, Arc::clone(&source));

        Ok(source)
    }

    pub(crate) fn generated_position(
        &mut self,
        path: &Path,
        position: LineCol,
    ) -> io::Result<LineCol> {
        let path = absolute(path).map_err(io::Error::other)?;
        self.load(&path)?;

        let Some(lowering) = self.lowerings.get(&path) else {
            return Ok(position);
        };

        let offset = usize::from(
            lowering
                .original
                .offset(position, PositionEncoding::Utf8)
                .map_err(io::Error::other)?,
        );

        let offset = lowering
            .mappings
            .iter()
            .find_map(|mapping| mapping.to_generated(offset))
            .ok_or_else(|| io::Error::other("graft mapping does not cover editor position"))?;

        lowering
            .generated
            .position(
                u32::try_from(offset).map_err(io::Error::other)?.into(),
                PositionEncoding::Utf8,
            )
            .map_err(io::Error::other)
    }

    pub(crate) fn original_range(
        &self,
        path: &Path,
        coordinates: [u32; 4],
    ) -> io::Result<[u32; 4]> {
        let path = absolute(path).map_err(io::Error::other)?;

        let Some(lowering) = self.lowerings.get(&path) else {
            return Ok(coordinates);
        };

        let map = |position: LineCol| -> io::Result<LineCol> {
            let offset = usize::from(
                lowering
                    .generated
                    .offset(position, PositionEncoding::Utf8)
                    .map_err(io::Error::other)?,
            );

            let offset = lowering
                .mappings
                .iter()
                .find_map(|mapping| mapping.to_original(offset))
                .ok_or_else(|| io::Error::other("graft mapping does not cover generated range"))?;

            lowering
                .original
                .position(
                    u32::try_from(offset).map_err(io::Error::other)?.into(),
                    PositionEncoding::Utf8,
                )
                .map_err(io::Error::other)
        };

        let start = map(LineCol {
            line: coordinates[0],
            col: coordinates[1],
        })?;

        let end = map(LineCol {
            line: coordinates[2],
            col: coordinates[3],
        })?;

        Ok([start.line, start.col, end.line, end.col])
    }

    pub(crate) fn entries(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        let mut paths = fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<io::Result<Vec<_>>>()?;

        paths.extend(self.sources.children(path).map_err(io::Error::other)?);
        paths.sort();
        paths.dedup();

        Ok(paths)
    }

    pub(crate) fn is_file(&self, path: &Path) -> io::Result<bool> {
        let identity = absolute(path).map_err(io::Error::other)?;

        if self.snapshots.contains_key(&identity)
            || self.sources.is_open(path).map_err(io::Error::other)?
        {
            return Ok(true);
        }

        Ok(metadata(path)?.is_some_and(|metadata| metadata.is_file()))
    }

    /// # Errors
    /// Returns filesystem, configuration and ambiguous-module failures.
    pub fn resolve(&mut self, from: &Path, specifier: &str) -> io::Result<Option<PathBuf>> {
        let extensions = self
            .configuration(from)?
            .frontend_extensions()
            .map(str::to_owned)
            .collect::<Vec<_>>();

        let Some((origin, target)) = self.namespace(from, specifier)? else {
            return Ok(None);
        };

        self.walk(&origin, &target, &extensions)
    }

    pub(crate) fn namespace(
        &mut self,
        from: &Path,
        specifier: &str,
    ) -> io::Result<Option<(PathBuf, PathBuf)>> {
        let from = absolute(from).map_err(io::Error::other)?;
        self.discovery.configurations(&from)?;

        let directory = from
            .parent()
            .ok_or_else(|| io::Error::other("module has no parent directory"))?;

        let base = if matches!(
            from.file_name().and_then(|name| name.to_str()),
            Some("init.lua" | "init.luau")
        ) {
            directory.parent().unwrap_or(directory)
        } else {
            directory
        };

        let specifier = specifier.replace('\\', "/");

        let (origin, target) = if specifier.starts_with("./") || specifier.starts_with("../") {
            (base.to_owned(), base.join(&specifier))
        } else if specifier.starts_with('@') {
            let mut context = from.clone();
            let mut value = specifier.clone();
            let mut tails = Vec::new();
            let mut seen = std::collections::BTreeSet::new();

            while let Some(rest) = value.strip_prefix('@') {
                let (name, tail) = rest.split_once('/').unwrap_or((rest, ""));
                tails.push(tail.trim_start_matches('/').to_owned());
                let name = name.to_ascii_lowercase();

                if name == "self" {
                    let node = if from.file_stem().is_some_and(|stem| stem == "init") {
                        directory.to_owned()
                    } else {
                        from.with_extension("")
                    };

                    context.clone_from(&from);

                    node.to_str()
                        .ok_or_else(|| io::Error::other("module identity requires UTF-8"))?
                        .clone_into(&mut value);

                    break;
                }

                let Some(alias) = self.discovery.alias(&context, &name)? else {
                    return Ok(None);
                };

                if !seen.insert((alias.configuration.clone(), name)) {
                    return Err(io::Error::other(format!(
                        "{}: cyclic alias in {specifier}",
                        alias.configuration.display()
                    )));
                }

                context = alias.configuration;
                value = alias.value.replace('\\', "/");
            }

            let origin = context
                .parent()
                .ok_or_else(|| io::Error::other("alias configuration has no parent"))?
                .to_owned();

            let mut target = origin.join(value);

            for tail in tails.into_iter().rev() {
                if !tail.is_empty() {
                    target.as_mut_os_string().push("/");
                    target.as_mut_os_string().push(tail);
                }
            }

            (origin, target)
        } else {
            return Ok(None);
        };

        Ok(Some((origin, target)))
    }

    fn walk(
        &self,
        origin: &Path,
        target: &Path,
        extensions: &[String],
    ) -> io::Result<Option<PathBuf>> {
        use std::path::Component;

        let (mut current, remaining) = match target.strip_prefix(origin) {
            Ok(remaining) => (origin.to_owned(), remaining),
            Err(_) => (PathBuf::new(), target),
        };

        let mut node = if current.as_os_str().is_empty() {
            Node {
                is_directory: true,
                module: None,
            }
        } else {
            let Some(node) = self.node(&current, extensions)? else {
                return Ok(None);
            };

            node
        };

        for component in remaining.components() {
            match component {
                Component::Prefix(_) | Component::RootDir => {
                    current.push(component);
                    continue;
                }

                Component::CurDir => continue,

                Component::ParentDir => {
                    if !current.pop() {
                        return Ok(None);
                    }
                }

                Component::Normal(_) => {
                    if !node.is_directory {
                        return Ok(None);
                    }

                    current
                        .as_mut_os_string()
                        .push(std::path::MAIN_SEPARATOR_STR);

                    current.as_mut_os_string().push(component.as_os_str());
                }
            }

            let Some(next) = self.node(&current, extensions)? else {
                return Ok(None);
            };

            node = next;
        }

        node.module
            .map(|module| absolute(&module).map_err(io::Error::other))
            .transpose()
    }

    fn node(&self, target: &Path, extensions: &[String]) -> io::Result<Option<Node>> {
        let mut candidates = Vec::new();

        if target.file_name().is_some_and(|name| name != "init") {
            for extension in ["luau", "lua"] {
                let path = target.with_added_extension(extension);

                if self.is_file(&path)? {
                    candidates.push(path);
                }
            }

            let mut generated = false;

            for path in &candidates {
                generated |= self.snapshots.contains_key(path)
                    || self.sources.is_open(path).map_err(io::Error::other)?;
            }

            if !generated {
                for extension in extensions {
                    let path = target.with_added_extension(extension);

                    if self.is_file(&path)? {
                        candidates.push(path);
                    }
                }
            }
        }

        let mut initializers = Vec::new();

        for extension in ["luau", "lua"] {
            let path = target.join("init").with_added_extension(extension);

            if self.is_file(&path)? {
                initializers.push(path);
            }
        }

        let mut generated = false;

        for path in &initializers {
            generated |= self.snapshots.contains_key(path)
                || self.sources.is_open(path).map_err(io::Error::other)?;
        }

        if !generated {
            for extension in extensions {
                let path = target.join("init").with_added_extension(extension);

                if self.is_file(&path)? {
                    initializers.push(path);
                }
            }
        }

        let directory = !initializers.is_empty()
            || self
                .sources
                .has_open_descendants(target)
                .map_err(io::Error::other)?
            || metadata(target)?.is_some_and(|metadata| metadata.is_dir());

        if candidates.len() > 1 || initializers.len() > 1 || (!candidates.is_empty() && directory) {
            return Err(io::Error::other(format!(
                "{}: ambiguous module",
                target.display()
            )));
        }

        let module = candidates.pop().or_else(|| initializers.pop());

        Ok((directory || module.is_some()).then_some(Node {
            is_directory: directory,
            module,
        }))
    }
}

fn metadata(path: &Path) -> io::Result<Option<fs::Metadata>> {
    match fs::metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),

        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }

        Err(error) => Err(io::Error::new(
            error.kind(),
            format!("{}: {error}", path.display()),
        )),
    }
}
