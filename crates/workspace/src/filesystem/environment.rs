use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fmt::Write as _,
    path::{Path, PathBuf},
};

use instar_configuration::environment::{Group, Security};
use instar_luau::{Class, Definition, Environment, Property};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{Error, Workspace, instances, model::Scope, paths};

#[derive(Default)]
pub(super) struct Registry {
    inputs: BTreeMap<Inputs, usize>,
    prepared: Vec<Prepared>,
    documentation: BTreeMap<Vec<(String, PathBuf)>, BTreeMap<String, Value>>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct Inputs {
    roblox: Option<(PathBuf, Security)>,
    capabilities: Option<PathBuf>,
    definitions: Vec<(String, PathBuf)>,
}

pub(super) struct Prepared {
    pub(super) native: Environment,
    pub(super) documentation: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    revision: String,
    services: Vec<String>,
    creatable_instances: Vec<String>,
    profiles: BTreeMap<String, String>,
    documentation: String,
    enumerations: String,
    properties: BTreeMap<String, Vec<Access>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    class: String,
    name: String,
    read: bool,
    write: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capabilities {
    classes: Vec<Capability>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Capability {
    name: String,
    service: bool,
    creatable: bool,
    properties: Vec<Permission>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Permission {
    name: String,
    read: bool,
    write: bool,
}

impl Registry {
    pub(super) fn dependencies(&self, dependencies: &mut BTreeSet<PathBuf>) {
        for inputs in self.inputs.keys() {
            dependencies.extend(inputs.capabilities.iter().cloned());
            dependencies.extend(inputs.definitions.iter().map(|(_, path)| path.clone()));

            if let Some((directory, _)) = &inputs.roblox {
                dependencies.insert(directory.clone());
                dependencies.insert(directory.join("metadata.json"));
                dependencies.insert(directory.join("documentation.json"));
            }
        }

        dependencies.extend(
            self.documentation
                .keys()
                .flatten()
                .map(|(_, path)| path.clone()),
        );

        dependencies.extend(
            self.prepared
                .iter()
                .flat_map(|prepared| &prepared.native.definitions)
                .map(|definition| PathBuf::from(&definition.name)),
        );
    }

    fn identity(&mut self, inputs: Inputs) -> Result<String, Error> {
        let index = match self.inputs.entry(inputs) {
            Entry::Occupied(entry) => *entry.get(),

            Entry::Vacant(entry) => {
                let prepared = prepare(entry.key())?;
                let index = self.prepared.len();
                self.prepared.push(prepared);
                entry.insert(index);

                index
            }
        };

        Ok(index.to_string())
    }

    pub(super) fn get(&self, name: &str) -> Option<&Prepared> {
        self.prepared.get(name.parse::<usize>().ok()?)
    }

    pub(super) fn documents(
        &mut self,
        groups: &[Group],
    ) -> Result<&BTreeMap<String, Value>, Error> {
        let paths = groups
            .iter()
            .flat_map(|group| {
                group
                    .documentation
                    .iter()
                    .map(|path| (group.namespace.clone(), path.clone()))
            })
            .collect();

        match self.documentation.entry(paths) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),

            Entry::Vacant(entry) => {
                let mut index = BTreeMap::new();

                for (namespace, path) in entry.key() {
                    documents(path, &paths::read(path)?, namespace, &mut index)?;
                }

                Ok(entry.insert(index))
            }
        }
    }
}

pub(super) fn identity(
    workspace: &Workspace,
    scope: &Scope,
    module: &Path,
) -> Result<Option<String>, Error> {
    let settings = &scope.configuration;

    let roblox = instances::enabled(workspace, scope, module)?;

    if let Some(identity) = scope.environments.borrow().get(&roblox) {
        return Ok(identity.clone());
    }

    let mut registry = workspace.environments.borrow_mut();
    registry.documents(&settings.environment.groups)?;

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
        if !roblox && definitions.is_empty() && settings.environment.capabilities.is_none() {
            None
        } else {
            let assets = if roblox {
                let directory = match &settings.environment.asset_directory {
                    Some(directory) => directory.clone(),

                    None => dirs::cache_dir()
                        .ok_or_else(|| Error::Path {
                            path: workspace.root.clone(),
                            reason: "the operating system cache directory is unavailable",
                        })?
                        .join("instar")
                        .join("roblox")
                        .join(env!("CARGO_PKG_VERSION")),
                };

                Some((paths::canonical(&directory)?, settings.environment.security))
            } else {
                None
            };

            Some(
                registry.identity(Inputs {
                    roblox: assets,
                    capabilities: settings
                        .environment
                        .capabilities
                        .as_deref()
                        .map(paths::canonical)
                        .transpose()?,
                    definitions,
                })?,
            )
        };

    scope
        .environments
        .borrow_mut()
        .insert(roblox, identity.clone());

    Ok(identity)
}

fn prepare(inputs: &Inputs) -> Result<Prepared, Error> {
    let mut prepared = Prepared {
        native: Environment::default(),
        documentation: BTreeMap::new(),
    };

    if let Some((directory, security)) = &inputs.roblox {
        assets(directory, *security, &mut prepared)?;
    }

    if let Some(path) = &inputs.capabilities {
        let capabilities: Capabilities = parse(path, &paths::read(path)?)?;

        prepared.native.classes = Some(
            capabilities
                .classes
                .into_iter()
                .map(|class| Class {
                    name: class.name,
                    service: class.service,
                    creatable: class.creatable,
                    properties: class
                        .properties
                        .into_iter()
                        .map(|property| Property {
                            name: property.name,
                            read: property.read,
                            write: property.write,
                        })
                        .collect(),
                })
                .collect(),
        );
    }

    for (namespace, path) in &inputs.definitions {
        prepared.native.definitions.push(Definition {
            name: paths::text(path)?,
            namespace: namespace.clone(),
            source: paths::read(path)?,
        });
    }

    Ok(prepared)
}

fn assets(directory: &Path, security: Security, prepared: &mut Prepared) -> Result<(), Error> {
    let metadata_path = directory.join("metadata.json");
    let metadata: Metadata = parse(&metadata_path, &paths::read(&metadata_path)?)?;

    if metadata.revision.len() != 40
        || !metadata
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(invalid(&metadata_path, "invalid asset revision"));
    }

    let profile = match security {
        Security::None => "none",
        Security::Local => "local",
        Security::Plugin => "plugin",
        Security::Roblox => "roblox",
    };

    let digest = metadata
        .profiles
        .get(profile)
        .ok_or_else(|| invalid(&metadata_path, "missing security profile"))?;

    let declarations = directory.join(format!("{profile}.d.luau"));
    let enumerations = directory.join("enumerations.d.luau");

    for (path, digest) in [
        (&declarations, digest),
        (&enumerations, &metadata.enumerations),
    ] {
        prepared.native.definitions.push(Definition {
            name: paths::text(path)?,
            namespace: "@roblox".to_owned(),
            source: verified(path, digest)?,
        });
    }

    let documentation = directory.join("documentation.json");

    documents(
        &documentation,
        &verified(&documentation, &metadata.documentation)?,
        "@roblox",
        &mut prepared.documentation,
    )?;

    let mut classes = BTreeMap::<String, Class>::new();

    for (names, service) in [
        (&metadata.services, true),
        (&metadata.creatable_instances, false),
    ] {
        for name in names {
            let class = classes.entry(name.clone()).or_insert_with(|| Class {
                name: name.clone(),
                service: false,
                creatable: false,
                properties: Vec::new(),
            });

            let permission = if service {
                &mut class.service
            } else {
                &mut class.creatable
            };

            if *permission {
                return Err(invalid(&metadata_path, "duplicate class capability"));
            }

            *permission = true;
        }
    }

    let properties = metadata
        .properties
        .get(profile)
        .ok_or_else(|| invalid(&metadata_path, "missing property profile"))?;

    for property in properties {
        let class = classes
            .entry(property.class.clone())
            .or_insert_with(|| Class {
                name: property.class.clone(),
                service: false,
                creatable: false,
                properties: Vec::new(),
            });

        if class
            .properties
            .iter()
            .any(|previous| previous.name == property.name)
        {
            return Err(invalid(&metadata_path, "duplicate property capability"));
        }

        class.properties.push(Property {
            name: property.name.clone(),
            read: property.read,
            write: property.write,
        });
    }

    prepared.native.classes = Some(classes.into_values().collect());

    Ok(())
}

fn verified(path: &Path, digest: &str) -> Result<String, Error> {
    let source = paths::read(path)?;
    let mut actual = String::with_capacity(64);

    for byte in Sha256::digest(source.as_bytes()) {
        write!(actual, "{byte:02x}").expect("writing to a string cannot fail");
    }

    if actual != digest {
        return Err(invalid(path, "asset contents do not match their metadata"));
    }

    Ok(source)
}

fn documents(
    path: &Path,
    source: &str,
    namespace: &str,
    target: &mut BTreeMap<String, Value>,
) -> Result<(), Error> {
    let documents: BTreeMap<String, Value> = parse(path, source)?;
    let prefix = format!("{namespace}/");

    if documents.keys().any(|symbol| !symbol.starts_with(&prefix)) {
        return Err(invalid(
            path,
            "documentation symbol is outside its declared namespace",
        ));
    }

    target.extend(documents);

    Ok(())
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
