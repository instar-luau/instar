use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    io,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_bridge::{Builtin, Class, Definition, Environment};

use serde::Deserialize;

use serde_json::Value;

use super::{Error, InvalidSource, SymbolOutcome, Workspace, instances, model::Scope, paths};

use crate::{
    assets::{self, Capability, Metadata},
    lint::Symbols,
    settings::environment::{Group, Security},
};

impl Workspace {
    /// Look up a configured documentation symbol in the module's environment.
    ///
    /// # Errors
    /// Returns unavailable, malformed, or inconsistent environment inputs.
    pub fn documentation(&self, module: &str, symbol: &str) -> Result<Option<Arc<Value>>, Error> {
        let scope = self.scope(Path::new(module))?;
        let mut registry = self.environments.borrow_mut();

        if let Some(documentation) = registry
            .documents(self, &scope.configuration.environment.groups)?
            .iter()
            .rev()
            .find_map(|documentation| documentation.get(symbol).cloned())
        {
            return Ok(Some(documentation));
        }

        let namespace = if symbol.starts_with("@luau/") {
            assets::Namespace::Luau
        } else if symbol.starts_with("@roblox/")
            && instances::is_enabled(self, &scope, Path::new(module))?
        {
            assets::Namespace::Roblox
        } else {
            return Ok(None);
        };

        Ok(registry.document(self, namespace)?.get(symbol).cloned())
    }

    /// Return service class names available in the module's Roblox environment.
    ///
    /// # Errors
    /// Returns unavailable or invalid configuration and environment inputs.
    pub fn services(&self, module: &str) -> Result<Vec<String>, Error> {
        let module = Path::new(module);
        let scope = self.scope(module)?;

        if !instances::is_enabled(self, &scope, module)? {
            return Ok(Vec::new());
        }

        let name = identity(self, &scope, module)?;
        let registry = self.environments.borrow();

        Ok(name
            .as_deref()
            .and_then(|name| registry.get(name))
            .and_then(|environment| environment.classes.as_deref())
            .into_iter()
            .flatten()
            .filter(|class| class.service)
            .map(|class| class.name.clone())
            .collect())
    }
}

#[derive(Default)]
pub(super) struct Registry {
    bundled_documentation: BTreeMap<assets::Namespace, (Arc<str>, Documentation)>,
    inputs: BTreeMap<Inputs, usize>,
    prepared: Vec<Prepared>,
    documentation: BTreeMap<Vec<PathBuf>, Vec<Documentation>>,
}

#[derive(Clone, Default)]
struct Prepared {
    environment: Environment,
    symbols: Arc<Symbols>,
    bundled: usize,
}

impl Registry {
    pub(super) fn dependencies(&self, dependencies: &mut BTreeSet<PathBuf>) {
        for inputs in self.inputs.keys() {
            dependencies.extend(inputs.capabilities.iter().cloned());
            dependencies.extend(inputs.definitions.iter().map(|(_, path)| path.clone()));

            if let Some(profile) = &inputs.roblox {
                dependencies.insert(profile.metadata.0.clone());
            }
        }

        dependencies.extend(self.documentation.keys().flatten().cloned());

        dependencies.extend(
            self.prepared
                .iter()
                .flat_map(|prepared| &prepared.environment.definitions)
                .map(|definition| PathBuf::from(&definition.name)),
        );
    }

    pub(super) fn get(&self, name: &str) -> Option<&Environment> {
        self.prepared
            .get(name.parse::<usize>().ok()?)
            .map(|prepared| &prepared.environment)
    }

    pub(super) fn symbols(
        &self,
        name: Option<&str>,
        globals: &[String],
        builtins: &[Builtin],
    ) -> SymbolOutcome {
        let mut symbols = Symbols::default();

        for builtin in builtins
            .iter()
            .filter(|builtin| !builtin.is_member && !builtin.is_type_function)
        {
            if builtin.is_type {
                symbols.types.insert(builtin.name.clone());
            } else {
                symbols.globals.insert(builtin.name.clone());
            }
        }

        if let Some(prepared) = name
            .and_then(|name| name.parse::<usize>().ok())
            .and_then(|index| self.prepared.get(index))
        {
            symbols
                .globals
                .extend(prepared.symbols.globals.iter().cloned());

            symbols.types.extend(prepared.symbols.types.iter().cloned());

            for definition in prepared
                .environment
                .definitions
                .iter()
                .skip(prepared.bundled)
            {
                if let Err(error) = symbols.declare(&definition.source) {
                    return SymbolOutcome::Invalid(InvalidSource {
                        path: PathBuf::from(&definition.name),
                        text: Arc::clone(&definition.source),
                        error,
                    });
                }
            }
        }

        symbols.globals.extend(globals.iter().cloned());

        SymbolOutcome::Ready(Arc::new(symbols))
    }

    pub(super) fn documents(
        &mut self,
        workspace: &Workspace,
        groups: &[Group],
    ) -> Result<&[Documentation], Error> {
        let paths = groups
            .iter()
            .flat_map(|group| group.documentation.iter().cloned())
            .collect();

        match self.documentation.entry(paths) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),

            Entry::Vacant(entry) => {
                workspace.observations.borrow_mut().take();
                let mut documents = Vec::new();

                for path in entry.key() {
                    documents.push(Documentation::new(path, &paths::read(path)?)?);
                }

                Ok(entry.insert(documents))
            }
        }
    }

    pub(super) fn document(
        &mut self,
        workspace: &Workspace,
        namespace: assets::Namespace,
    ) -> Result<&Documentation, Error> {
        let (path, source) = workspace.assets.load(
            namespace,
            "documentation.json",
            &workspace.cancellation.borrow(),
        )?;

        if self
            .bundled_documentation
            .get(&namespace)
            .is_none_or(|(cached, _)| !Arc::ptr_eq(cached, &source))
        {
            let document = Documentation::new(&path, &source)?;

            self.bundled_documentation
                .insert(namespace, (source, document));
        }

        Ok(&self.bundled_documentation[&namespace].1)
    }

    fn identity(
        &mut self,
        workspace: &Workspace,
        inputs: Inputs,
        is_cancelled: &dyn Fn() -> bool,
    ) -> Result<String, Error> {
        let index = match self.inputs.entry(inputs) {
            Entry::Occupied(entry) => *entry.get(),

            Entry::Vacant(entry) => {
                workspace.observations.borrow_mut().take();

                let prepared = if let Some(profile) = &entry.key().roblox {
                    profile.prepare(is_cancelled)?
                } else {
                    Prepared::default()
                };

                let prepared = prepare(entry.key(), prepared)?;
                let index = self.prepared.len();
                self.prepared.push(prepared);
                entry.insert(index);

                index
            }
        };

        Ok(index.to_string())
    }
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Inputs {
    roblox: Option<Profile>,
    capabilities: Option<PathBuf>,
    definitions: Vec<(String, PathBuf)>,
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Profile {
    name: &'static str,
    metadata: (PathBuf, Arc<str>),
    declarations: (PathBuf, Arc<str>),
}

impl Profile {
    fn prepare(&self, is_cancelled: &dyn Fn() -> bool) -> Result<Prepared, Error> {
        super::process::interrupted(is_cancelled)?;

        let metadata_path = &self.metadata.0;
        let mut metadata: Metadata = parse(metadata_path, &self.metadata.1)?;

        let profile = metadata
            .profiles
            .remove(self.name)
            .ok_or_else(|| invalid(metadata_path, "Missing security profile."))?;

        let declarations = &self.declarations.0;

        let definition = Definition {
            name: paths::text(declarations)?,
            namespace: "@roblox".to_owned(),
            source: Arc::clone(&self.declarations.1),
        };

        let mut symbols = Symbols::default();

        symbols
            .declare(&definition.source)
            .map_err(|source| Error::Filesystem {
                path: declarations.clone(),
                source: io::Error::new(io::ErrorKind::InvalidData, source),
            })?;

        symbols.globals.insert("Enum".to_owned());

        symbols
            .types
            .extend(["Enum", "EnumItem", "Enumerations"].map(str::to_owned));

        let prepared = Prepared {
            environment: Environment {
                definitions: vec![definition],
                enumerations: Some(metadata.enumerations),
                classes: Some(
                    profile
                        .classes
                        .into_iter()
                        .map(Class::from)
                        .collect::<Vec<_>>()
                        .into(),
                ),
            },
            symbols: symbols.into(),
            bundled: 1,
        };

        Ok(prepared)
    }
}

pub(super) struct Documentation {
    index: BTreeMap<String, Arc<Value>>,
}

impl Documentation {
    fn new(path: &Path, source: &str) -> Result<Self, Error> {
        let index: BTreeMap<String, Arc<Value>> = parse(path, source)?;

        Ok(Self { index })
    }

    pub(super) fn get(&self, symbol: &str) -> Option<&Arc<Value>> {
        self.index.get(symbol)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    classes: Vec<Capability>,
}

pub(super) fn identity(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<Option<String>, Error> {
    let settings = &scope.configuration;

    let is_roblox = instances::is_enabled(workspace, scope, module)?;

    let mut registry = workspace.environments.borrow_mut();

    let definitions: Vec<_> = settings
        .environment
        .groups
        .iter()
        .flat_map(|group| {
            group
                .definitions
                .iter()
                .map(|path| Ok((group.namespace.clone(), paths::canonical(path)?)))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let identity =
        if !is_roblox && definitions.is_empty() && settings.environment.capabilities.is_none() {
            None
        } else {
            let assets = if is_roblox {
                let (name, filename) = match settings.environment.security {
                    Security::None => ("none", "none.d.luau"),
                    Security::Local => ("local", "local.d.luau"),
                    Security::Plugin => ("plugin", "plugin.d.luau"),
                    Security::Roblox => ("roblox", "roblox.d.luau"),
                };

                Some(Profile {
                    name,
                    metadata: workspace.assets.load(
                        assets::Namespace::Roblox,
                        "metadata.json",
                        &workspace.cancellation.borrow(),
                    )?,
                    declarations: workspace.assets.load(
                        assets::Namespace::Roblox,
                        filename,
                        &workspace.cancellation.borrow(),
                    )?,
                })
            } else {
                None
            };

            Some(
                registry.identity(
                    workspace,
                    Inputs {
                        roblox: assets,
                        capabilities: settings
                            .environment
                            .capabilities
                            .as_deref()
                            .map(paths::canonical)
                            .transpose()?,
                        definitions,
                    },
                    &|| workspace.cancellation.borrow().is_cancelled(),
                )?,
            )
        };

    Ok(identity)
}

fn prepare(inputs: &Inputs, mut prepared: Prepared) -> Result<Prepared, Error> {
    if let Some(path) = &inputs.capabilities {
        let capabilities: Capabilities = parse(path, &paths::read(path)?)?;

        prepared.environment.classes = Some(
            capabilities
                .classes
                .into_iter()
                .map(Class::from)
                .collect::<Vec<_>>()
                .into(),
        );
    }

    for (namespace, path) in &inputs.definitions {
        prepared.environment.definitions.push(Definition {
            name: paths::text(path)?,
            namespace: namespace.clone(),
            source: paths::read(path)?.into(),
        });
    }

    Ok(prepared)
}

fn parse<'source, Output: Deserialize<'source>>(
    path: &Path,
    source: &'source str,
) -> Result<Output, Error> {
    serde_json::from_str(source).map_err(|source| Error::Data {
        path: path.to_owned(),
        source,
    })
}

fn invalid(path: &Path, reason: &'static str) -> Error {
    Error::Path {
        path: path.to_owned(),
        reason,
    }
}
