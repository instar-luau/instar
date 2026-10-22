use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use instar_bridge::{
    Configuration, Environment, Flag, Host, HostError, Instance, Language, Mode, Navigation,
    Operation,
};

use super::{Error, Workspace, environment, instances, paths};
use crate::settings::{Configuration as ProjectConfiguration, Flag as ConfigurationFlag, check};

impl Host for Workspace {
    fn alias(&self, module: &str, name: &str) -> Result<Navigation, HostError> {
        let scope = self
            .scope(Path::new(module))
            .map_err(|error| failure(module, Operation::Alias, error))?;

        let Some(target) = scope.configuration.resolve.aliases.get(name) else {
            return Ok(Navigation::Missing);
        };

        logical(target)
            .and_then(|path| resolve(self, &path))
            .map_err(|error| failure(module, Operation::Alias, error))
    }

    fn aliases(&self, module: &str) -> Result<Vec<String>, HostError> {
        self.scope(Path::new(module))
            .map(|scope| {
                scope
                    .configuration
                    .resolve
                    .aliases
                    .keys()
                    .cloned()
                    .collect()
            })
            .map_err(|error| failure(module, Operation::Aliases, error))
    }

    fn child(&self, module: &str, name: &str) -> Result<Navigation, HostError> {
        let mut components = Path::new(name).components();

        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Ok(Navigation::Missing);
        }

        logical(Path::new(module))
            .and_then(|path| resolve(self, &path.join(name)))
            .map_err(|error| failure(module, Operation::Child, error))
    }

    fn children(&self, module: &str) -> Result<Vec<String>, HostError> {
        children(self, Path::new(module))
            .map_err(|error| failure(module, Operation::Children, error))
    }

    fn configuration(&self, module: &str) -> Result<Configuration, HostError> {
        let scope = self
            .scope(Path::new(module))
            .map_err(|error| failure(module, Operation::Configuration, error))?;

        let configuration = &scope.configuration;

        let mode = match configuration.check.mode {
            check::Mode::Strict => Mode::Strict,
            check::Mode::NonStrict => Mode::NonStrict,
            check::Mode::NoCheck => Mode::NoCheck,
        };

        let locations = instances::configuration(self, &scope, Path::new(module))
            .map_err(|error| failure(module, Operation::Hierarchy, error))?;

        Ok(Configuration {
            mode,
            globals: configuration.environment.globals.clone(),
            environment: environment::identity(self, &scope, Path::new(module))
                .map_err(|error| failure(module, Operation::Environment, error))?,
            flags: flags(configuration),
            locations,
            type_errors: scope.language.as_ref().is_none_or(Language::type_errors),
        })
    }

    fn environment(&self, name: &str) -> Result<Environment, HostError> {
        let environments = self.environments.borrow();

        let prepared = environments.get(name).ok_or_else(|| HostError {
            module: name.to_owned(),
            operation: Operation::Environment,
            source: "Unknown declaration environment.".into(),
        })?;

        self.snapshots
            .borrow_mut()
            .extend(prepared.definitions.iter().map(|definition| {
                (
                    definition.name.clone(),
                    Some(Arc::clone(&definition.source)),
                )
            }));

        Ok(prepared.clone())
    }

    fn flags(&self) -> Result<BTreeMap<String, Flag>, HostError> {
        Ok(flags(&self.root().configuration))
    }

    fn hierarchy(&self, name: &str) -> Result<Vec<Instance>, HostError> {
        self.instances
            .borrow()
            .get(name)
            .map(<[Instance]>::to_vec)
            .ok_or_else(|| HostError {
                module: name.to_owned(),
                operation: Operation::Hierarchy,
                source: "Unknown instance hierarchy.".into(),
            })
    }

    fn parent(&self, module: &str) -> Result<Navigation, HostError> {
        let parent = || {
            let path = logical(Path::new(module))?;

            let Some(parent) = path.parent() else {
                return Ok(Navigation::Missing);
            };

            if !paths::metadata(parent)?.is_some_and(|metadata| metadata.is_dir()) {
                return Ok(Navigation::Missing);
            }

            match directory(self, parent)? {
                Navigation::Ambiguous => found(parent),
                navigation => Ok(navigation),
            }
        };

        parent().map_err(|error| failure(module, Operation::Parent, error))
    }

    fn source(&self, module: &str) -> Result<Option<String>, HostError> {
        Self::source(self, module)
            .map(|source| source.map(|source| source.to_string()))
            .map_err(|error| failure(module, Operation::Source, error))
    }
}

pub(super) fn flags(configuration: &ProjectConfiguration) -> BTreeMap<String, Flag> {
    configuration
        .flags
        .iter()
        .map(|(name, flag)| {
            let flag = match flag {
                ConfigurationFlag::Boolean(is_true) => Flag::Boolean(*is_true),
                ConfigurationFlag::Integer(value) => Flag::Integer(*value),
            };

            (name.clone(), flag)
        })
        .collect()
}

pub(super) fn logical(path: &Path) -> Result<PathBuf, Error> {
    if paths::is_source(path) && !paths::metadata(path)?.is_some_and(|metadata| metadata.is_dir()) {
        if path.file_stem().is_some_and(|stem| stem == "init") {
            return Ok(path.parent().unwrap_or(path).to_owned());
        }

        return Ok(path.with_extension(""));
    }

    Ok(path.to_owned())
}

fn children(workspace: &Workspace, module: &Path) -> Result<Vec<String>, Error> {
    let directory = logical(module)?;

    workspace
        .observed_paths
        .borrow_mut()
        .insert(directory.clone());

    if !paths::metadata(&directory)?.is_some_and(|metadata| metadata.is_dir()) {
        return Ok(Vec::new());
    }

    let entries = fs::read_dir(&directory)
        .and_then(|entries| {
            entries
                .map(|entry| entry.map(|entry| entry.path()))
                .collect::<Result<BTreeSet<_>, _>>()
        })
        .map_err(|source| Error::Filesystem {
            path: directory.clone(),
            source,
        })?;

    let sources = workspace.sources.borrow();
    let mut names = BTreeSet::new();

    for entry in entries.iter().map(PathBuf::as_path).chain(
        sources
            .keys()
            .map(Path::new)
            .filter(|path| path.parent() == Some(directory.as_path())),
    ) {
        if workspace.cancellation.borrow().is_cancelled() {
            return Err(Error::Cancelled);
        }

        let name = if paths::metadata(entry)?.is_some_and(|metadata| metadata.is_dir()) {
            entry.file_name()
        } else if paths::is_source(entry) {
            entry.file_stem().filter(|name| *name != "init")
        } else {
            None
        };

        if let Some(name) = name {
            names.insert(paths::text(Path::new(name))?);
        }
    }

    Ok(names.into_iter().collect())
}

fn resolve(workspace: &Workspace, path: &Path) -> Result<Navigation, Error> {
    workspace
        .observed_paths
        .borrow_mut()
        .insert(path.to_owned());

    let mut candidate = None;

    if path.file_name().is_none_or(|name| name != "init") {
        for suffix in [".luau", ".lua"] {
            let mut file = path.as_os_str().to_owned();
            file.push(suffix);
            let file = PathBuf::from(file);
            workspace.observed_paths.borrow_mut().insert(file.clone());

            if is_source(workspace, &file)? {
                if candidate.is_some() {
                    return Ok(Navigation::Ambiguous);
                }

                candidate = Some(file);
            }
        }
    }

    if paths::metadata(path)?.is_some_and(|metadata| metadata.is_dir()) {
        if candidate.is_some() {
            return Ok(Navigation::Ambiguous);
        }

        return directory(workspace, path);
    }

    candidate.as_deref().map_or(Ok(Navigation::Missing), found)
}

fn directory(workspace: &Workspace, path: &Path) -> Result<Navigation, Error> {
    let mut candidate = None;

    for name in ["init.luau", "init.lua"] {
        let file = path.join(name);
        workspace.observed_paths.borrow_mut().insert(file.clone());

        if is_source(workspace, &file)? {
            if candidate.is_some() {
                return Ok(Navigation::Ambiguous);
            }

            candidate = Some(file);
        }
    }

    found(candidate.as_deref().unwrap_or(path))
}

fn is_source(workspace: &Workspace, path: &Path) -> Result<bool, Error> {
    if paths::metadata(path)?.is_some_and(|metadata| metadata.is_file()) {
        return Ok(true);
    }

    if !path
        .parent()
        .map(paths::metadata)
        .transpose()?
        .flatten()
        .is_some_and(|metadata| metadata.is_dir())
    {
        return Ok(false);
    }

    let module = paths::text(&paths::identity(path)?)?;

    Ok(workspace.sources.borrow().contains_key(&module))
}

fn found(path: &Path) -> Result<Navigation, Error> {
    paths::text(&paths::identity(path)?).map(Navigation::Found)
}

fn failure(module: &str, operation: Operation, source: Error) -> HostError {
    HostError {
        module: module.to_owned(),
        operation,
        source: Box::new(source),
    }
}
