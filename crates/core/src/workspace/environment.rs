use std::{
    cell::OnceCell,
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fmt::{self, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_luau::{Class, Definition, Environment, Property};

use serde::{
    Deserialize, Deserializer,
    de::{DeserializeSeed, MapAccess, SeqAccess, Visitor},
};

use serde_json::Value;
use sha2::{Digest as _, Sha256};

use super::{Error, Workspace, assets, instances, model::Scope, paths};

use crate::{
    assets::{Documentation as DocumentationMetadata, Metadata},
    lint::Symbols,
    settings::environment::{Group, Security},
};

#[derive(Default)]
pub(super) struct Registry {
    assets: BTreeMap<assets::Namespace, PathBuf>,
    builtin_documentation: BTreeMap<assets::Namespace, Documentation>,
    inputs: BTreeMap<Inputs, usize>,
    prepared: Vec<Prepared>,
    documentation: BTreeMap<Vec<(String, PathBuf)>, Vec<Documentation>>,
    symbols: BTreeMap<(Option<String>, Vec<String>), Arc<Symbols>>,
}

impl Registry {
    pub(super) fn dependencies(&self, dependencies: &mut BTreeSet<PathBuf>) {
        for directory in self.assets.values() {
            dependencies.insert(directory.clone());
            dependencies.insert(directory.join("metadata.json"));
            dependencies.insert(directory.join("documentation.json"));

            if let Some(parent) = directory.parent() {
                dependencies.insert(parent.join("current"));
            }
        }

        for inputs in self.inputs.keys() {
            dependencies.extend(inputs.capabilities.iter().cloned());
            dependencies.extend(inputs.definitions.iter().map(|(_, path)| path.clone()));
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

    pub(super) fn get(&self, name: &str) -> Option<&Prepared> {
        self.prepared.get(name.parse::<usize>().ok()?)
    }

    pub(super) fn symbols(
        &mut self,
        name: Option<&str>,
        globals: &[String],
        builtins: &str,
    ) -> Result<Arc<Symbols>, Error> {
        let key = (name.map(str::to_owned), globals.to_owned());

        if let Some(symbols) = self.symbols.get(&key) {
            return Ok(Arc::clone(symbols));
        }

        let mut symbols = Symbols::default();

        symbols.declare(builtins).map_err(|source| Error::Syntax {
            path: PathBuf::from("<builtins>"),
            source,
        })?;

        if let Some(prepared) = name.and_then(|name| self.get(name)) {
            if prepared.native.enumerations.is_some() {
                symbols.globals.insert("Enum".to_owned());

                symbols
                    .types
                    .extend(["Enum", "EnumItem", "Enumerations"].map(str::to_owned));
            }

            for definition in &prepared.native.definitions {
                symbols
                    .declare(&definition.source)
                    .map_err(|source| Error::Syntax {
                        path: PathBuf::from(&definition.name),
                        source,
                    })?;
            }
        }

        symbols.globals.extend(globals.iter().cloned());
        let symbols = Arc::new(symbols);
        self.symbols.insert(key, Arc::clone(&symbols));

        Ok(symbols)
    }

    pub(super) fn documents(&mut self, groups: &[Group]) -> Result<&[Documentation], Error> {
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
                let mut documents = Vec::new();

                for (namespace, path) in entry.key() {
                    documents.push(Documentation::new(path, paths::read(path)?, namespace)?);
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
        let directory = self.directory(workspace, namespace)?;

        match self.builtin_documentation.entry(namespace) {
            Entry::Occupied(entry) => Ok(entry.into_mut()),
            Entry::Vacant(entry) => Ok(entry.insert(documentation(&directory, namespace)?)),
        }
    }

    fn directory(
        &mut self,
        workspace: &Workspace,
        namespace: assets::Namespace,
    ) -> Result<PathBuf, Error> {
        match self.assets.entry(namespace) {
            Entry::Occupied(entry) => Ok(entry.get().clone()),

            Entry::Vacant(entry) => {
                let directory = paths::canonical(&assets::directory(workspace, namespace)?)?;

                Ok(entry.insert(directory).clone())
            }
        }
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
}

#[derive(Eq, Ord, PartialEq, PartialOrd)]
struct Inputs {
    roblox: Option<(PathBuf, Security)>,
    capabilities: Option<PathBuf>,
    definitions: Vec<(String, PathBuf)>,
}

pub(super) struct Prepared {
    pub(super) native: Environment,
}

pub(super) struct Documentation {
    source: String,
    index: OnceCell<BTreeMap<String, Value>>,
}

impl Documentation {
    fn new(path: &Path, source: String, namespace: &str) -> Result<Self, Error> {
        let mut decoder = serde_json::Deserializer::from_str(&source);

        let valid = decoder
            .deserialize_map(Namespace(&format!("{namespace}/")))
            .and_then(|valid| decoder.end().map(|()| valid))
            .map_err(|source| Error::Data {
                path: path.to_owned(),
                source,
            })?;

        if !valid {
            return Err(invalid(
                path,
                "documentation symbol is outside its declared namespace",
            ));
        }

        Ok(Self {
            source,
            index: OnceCell::new(),
        })
    }

    pub(super) fn get(&self, symbol: &str) -> Option<&Value> {
        self.index
            .get_or_init(|| {
                serde_json::from_str(&self.source)
                    .expect("internal error: invalid validated documentation")
            })
            .get(symbol)
    }
}

struct Namespace<'source>(&'source str);

impl<'source> DeserializeSeed<'source> for Namespace<'_> {
    type Value = bool;

    fn deserialize<Decoder: Deserializer<'source>>(
        self,
        decoder: Decoder,
    ) -> Result<bool, Decoder::Error> {
        decoder.deserialize_any(self)
    }
}

impl<'source> Visitor<'source> for Namespace<'_> {
    type Value = bool;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a map")
    }

    fn visit_bool<Failure>(self, _: bool) -> Result<bool, Failure> {
        Ok(true)
    }

    fn visit_i64<Failure>(self, _: i64) -> Result<bool, Failure> {
        Ok(true)
    }

    fn visit_u64<Failure>(self, _: u64) -> Result<bool, Failure> {
        Ok(true)
    }

    fn visit_f64<Failure>(self, _: f64) -> Result<bool, Failure> {
        Ok(true)
    }

    fn visit_str<Failure>(self, value: &str) -> Result<bool, Failure> {
        Ok(value.starts_with(self.0))
    }

    fn visit_unit<Failure>(self) -> Result<bool, Failure> {
        Ok(true)
    }

    fn visit_seq<Sequence: SeqAccess<'source>>(
        self,
        mut sequence: Sequence,
    ) -> Result<bool, Sequence::Error> {
        while sequence.next_element_seed(Namespace(""))?.is_some() {}

        Ok(true)
    }

    fn visit_map<Map: MapAccess<'source>>(self, mut map: Map) -> Result<bool, Map::Error> {
        let mut valid = true;

        while let Some(validated) = map.next_key_seed(Namespace(self.0))? {
            valid &= validated;
            map.next_value_seed(Namespace(""))?;
        }

        Ok(valid)
    }
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
                let directory = registry.directory(workspace, assets::Namespace::Roblox)?;

                Some((directory, settings.environment.security))
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

pub(super) fn validate(
    directory: &Path,
    namespace: assets::Namespace,
    cancelled: fn() -> bool,
) -> Result<(), Error> {
    super::process::interrupted(cancelled)?;
    documentation(directory, namespace)?;

    if namespace == assets::Namespace::Luau {
        return Ok(());
    }

    for security in [
        Security::None,
        Security::Local,
        Security::Plugin,
        Security::Roblox,
    ] {
        if cancelled() {
            return Err(Error::Cancelled);
        }

        assets(
            directory,
            security,
            &mut Prepared {
                native: Environment::default(),
            },
        )?;
    }

    Ok(())
}

fn assets(directory: &Path, security: Security, prepared: &mut Prepared) -> Result<(), Error> {
    let metadata_path = directory.join("metadata.json");
    let metadata: Metadata = parse(&metadata_path, &paths::read(&metadata_path)?)?;

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

    prepared.native.definitions.push(Definition {
        name: paths::text(&declarations)?,
        namespace: "@roblox".to_owned(),
        source: verified(&declarations, digest)?,
    });

    prepared.native.enumerations = Some(metadata.enumerations);

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

fn documentation(directory: &Path, namespace: assets::Namespace) -> Result<Documentation, Error> {
    let metadata_path = directory.join("metadata.json");
    let source = paths::read(&metadata_path)?;

    let digest = match namespace {
        assets::Namespace::Luau => {
            let metadata: DocumentationMetadata = parse(&metadata_path, &source)?;

            metadata.documentation
        }

        assets::Namespace::Roblox => {
            let metadata: Metadata = parse(&metadata_path, &source)?;

            metadata.documentation
        }
    };

    let path = directory.join("documentation.json");

    Documentation::new(
        &path,
        verified(&path, &digest)?,
        &format!("@{}", namespace.name()),
    )
}

fn verified(path: &Path, digest: &str) -> Result<String, Error> {
    let source = paths::read(path)?;
    let mut actual = String::with_capacity(64);

    for byte in Sha256::digest(source.as_bytes()) {
        write!(actual, "{byte:02x}").expect("internal error: cannot write to a string");
    }

    if actual != digest {
        return Err(invalid(path, "asset contents do not match their metadata"));
    }

    Ok(source)
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
