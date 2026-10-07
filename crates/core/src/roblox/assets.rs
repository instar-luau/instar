//! Automatically downloaded and verified Roblox environments.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend::{Class, Property};
use serde::Deserialize;
use ureq::Agent;

use super::members::Members;

use crate::{
    assets::{self, HOST, fetch, install, verify},
    configuration::Security,
    documentation::Index,
    project::{Change, Project},
    source::Document,
};

const PROFILES: [&str; 4] = ["none", "local", "plugin", "roblox"];

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

impl Metadata {
    fn parse(source: &str) -> io::Result<Self> {
        let metadata: Self = assets::parse(source)?;

        assets::revision(&metadata.revision)?;

        names(&metadata.services)?;
        names(&metadata.creatable_instances)?;

        for profile in PROFILES {
            if !metadata.profiles.contains_key(profile)
                || !metadata.properties.contains_key(profile)
            {
                return Err(invalid("missing Roblox security profile"));
            }
        }

        Ok(metadata)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Access {
    class: String,
    name: String,
    read: bool,
    write: bool,
}

pub(crate) struct Assets {
    pub(crate) definitions: Vec<(PathBuf, Document)>,
    pub(crate) classes: Arc<[Class]>,
    pub(crate) documentation: Index,
    pub(crate) members: Members,
}

impl Project {
    pub(crate) fn roblox_assets(
        &mut self,
        security: Security,
        options: &Options,
        started: Instant,
    ) -> io::Result<Rc<Assets>> {
        let directory = cache_directory()?;

        match self.source(&directory.join("metadata.json")) {
            Ok(_) => tracing::debug!(path = %directory.display(), "Using cached Roblox assets"),

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let files = fetch(
                    format!("{HOST}/roblox"),
                    download,
                    &options.remaining(started),
                )?;

                install(&directory, files, options, started)?;

                self.view
                    .change(&directory, Change::Disk(directory.clone()));
            }

            Err(error) => return Err(error),
        }

        self.load_assets(&directory, security)
    }

    pub(crate) fn member(&mut self, directory: &Path, class: &str, name: &str) -> io::Result<bool> {
        let settings = self.configuration(directory)?;

        let (options, started) = self
            .operation
            .clone()
            .unwrap_or_else(|| (Options::new(self.timeout), Instant::now()));

        let assets =
            self.roblox_assets(settings.configuration.roblox.security, &options, started)?;

        assets.members.contains(class, name)
    }

    pub(crate) fn load_assets(
        &mut self,
        directory: &Path,
        security: Security,
    ) -> io::Result<Rc<Assets>> {
        let metadata = self.source(&directory.join("metadata.json"))?;

        let profile = match security {
            Security::None => "none",
            Security::Local => "local",
            Security::Plugin => "plugin",
            Security::Roblox => "roblox",
        };

        let path = directory.join(format!("{profile}.d.luau"));
        let document = self.source(&path)?;
        let documentation = self.source(&directory.join("documentation.json"))?;
        let enumeration_path = directory.join("enumerations.d.luau");
        let enumerations = self.source(&enumeration_path)?;

        let revisions = [
            metadata.revision,
            document.revision,
            documentation.revision,
            enumerations.revision,
        ];

        if let Some(assets) = self.roblox.get(&revisions) {
            return Ok(Rc::clone(assets));
        }

        let metadata = Metadata::parse(&metadata.text)?;

        let expected = metadata
            .profiles
            .get(profile)
            .ok_or_else(|| invalid("missing Roblox security profile hash"))?;

        verify(&document.text, expected)?;
        verify(&documentation.text, &metadata.documentation)?;

        let documentation = Index::parse(&documentation.text, &["@roblox"])?;

        let services = names(&metadata.services)?;
        let creatable = names(&metadata.creatable_instances)?;

        let mut classes = services
            .union(&creatable)
            .map(|name| {
                (
                    (*name).to_owned(),
                    Class {
                        name: (*name).to_owned(),
                        service: services.contains(name),
                        creatable: creatable.contains(name),
                        properties: Vec::new(),
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();

        let access = metadata
            .properties
            .get(profile)
            .ok_or_else(|| invalid("missing Roblox property access profile"))?;

        for property in access {
            let class = classes
                .entry(property.class.clone())
                .or_insert_with(|| Class {
                    name: property.class.clone(),
                    service: false,
                    creatable: false,
                    properties: Vec::new(),
                });

            class.properties.push(Property {
                name: property.name.clone(),
                read: property.read,
                write: property.write,
            });
        }

        verify(&enumerations.text, &metadata.enumerations)?;
        let members = Members::parse(&document.text)?;

        let assets = Rc::new(Assets {
            definitions: vec![(path, document), (enumeration_path, enumerations)],
            classes: classes.into_values().collect(),
            documentation,
            members,
        });

        self.roblox.insert(revisions, Rc::clone(&assets));

        Ok(assets)
    }
}

/// Returns the versioned, automatically managed Roblox asset cache.
///
/// # Errors
/// Returns an error when the operating system's user cache directory is unavailable.
pub fn cache_directory() -> io::Result<PathBuf> {
    assets::directory("roblox")
}

fn names(values: &[String]) -> io::Result<BTreeSet<&str>> {
    let mut names = BTreeSet::new();

    for value in values {
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            || !names.insert(value.as_str())
        {
            return Err(invalid("invalid or duplicate Roblox metadata class"));
        }
    }

    Ok(names)
}

pub(crate) fn download(url: &str, options: &Options) -> io::Result<BTreeMap<String, String>> {
    let started = Instant::now();

    let agent = Agent::config_builder()
        .https_only(url.starts_with("https://"))
        .build()
        .new_agent();

    let get = |name: &str| assets::get(&agent, &format!("{url}/{name}"), options, started);

    let source = get("metadata.json")?;
    let metadata = Metadata::parse(&source)?;
    let mut files = BTreeMap::new();

    for profile in PROFILES {
        let name = format!("{profile}.d.luau");
        let text = get(&name)?;
        verify(&text, &metadata.profiles[profile])?;
        files.insert(name, text);
    }

    for (name, hash) in [
        ("enumerations.d.luau", &metadata.enumerations),
        ("documentation.json", &metadata.documentation),
    ] {
        let text = get(name)?;
        verify(&text, hash)?;

        if name == "documentation.json" {
            Index::parse(&text, &["@roblox"])?;
        }

        files.insert(name.to_owned(), text);
    }

    files.insert("metadata.json".to_owned(), source);

    Ok(files)
}
