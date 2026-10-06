//! Automatically downloaded and verified Roblox environments.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use instar_analysis::{Options, Reason, error::invalid};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{
    configuration::Security,
    project::{Change, Project},
    source::Document,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
const HOST: &str = "https://instar-luau.github.io/instar/roblox";
const PROFILES: [&str; 4] = ["none", "local", "plugin", "roblox"];
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Returns the versioned, automatically managed Roblox asset cache.
///
/// # Errors
/// Returns an error when the operating system's user cache directory is unavailable.
pub fn cache_directory() -> io::Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user cache directory is unavailable",
        )
    })?;

    if !base.is_absolute() {
        return Err(invalid("user cache directory must be absolute"));
    }

    Ok(base.join("instar").join("roblox").join(VERSION))
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

impl Metadata {
    fn parse(source: &str) -> io::Result<Self> {
        let metadata: Self = serde_json::from_str(source).map_err(invalid)?;

        if metadata.revision.len() != 40
            || !metadata
                .revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(invalid("invalid Roblox tracker revision"));
        }

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
    pub(crate) classes: Vec<instar_bridge::frontend::Class>,
}

impl Project {
    pub(crate) fn roblox_assets(
        &mut self,
        security: Security,
        options: &Options,
        started: Instant,
    ) -> io::Result<Assets> {
        let directory = cache_directory()?;

        match self.source(&directory.join("metadata.json")) {
            Ok(_) => {}

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let files = fetch(format!("{HOST}/{VERSION}"), &options.remaining(started))?;
                interrupted(options, started)?;
                install(&directory, files)?;

                self.view
                    .change(&directory, Change::Disk(directory.clone()));
            }

            Err(error) => return Err(error),
        }

        self.load_assets(&directory, security)
    }

    fn load_assets(&mut self, directory: &Path, security: Security) -> io::Result<Assets> {
        let metadata = self.source(&directory.join("metadata.json"))?;
        let metadata = Metadata::parse(&metadata.text)?;

        let profile = match security {
            Security::None => "none",
            Security::Local => "local",
            Security::Plugin => "plugin",
            Security::Roblox => "roblox",
        };

        let path = directory.join(format!("{profile}.d.luau"));
        let document = self.source(&path)?;

        let expected = metadata
            .profiles
            .get(profile)
            .ok_or_else(|| invalid("missing Roblox security profile hash"))?;

        verify(&document.text, expected)?;
        let documentation = self.source(&directory.join("documentation.json"))?;
        verify(&documentation.text, &metadata.documentation)?;

        let documentation: serde_json::Value =
            serde_json::from_str(&documentation.text).map_err(invalid)?;

        if !documentation.is_object() {
            return Err(invalid("Roblox documentation must be an object"));
        }

        let services = names(&metadata.services)?;
        let creatable = names(&metadata.creatable_instances)?;

        let mut classes = services
            .union(&creatable)
            .map(|name| {
                (
                    (*name).to_owned(),
                    instar_bridge::frontend::Class {
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
            let class = classes.entry(property.class.clone()).or_insert_with(|| {
                instar_bridge::frontend::Class {
                    name: property.class.clone(),
                    service: false,
                    creatable: false,
                    properties: Vec::new(),
                }
            });

            class.properties.push(instar_bridge::frontend::Property {
                name: property.name.clone(),
                read: property.read,
                write: property.write,
            });
        }

        let enumeration_path = directory.join("enumerations.d.luau");
        let enumerations = self.source(&enumeration_path)?;
        verify(&enumerations.text, &metadata.enumerations)?;

        Ok(Assets {
            definitions: vec![(path, document), (enumeration_path, enumerations)],
            classes: classes.into_values().collect(),
        })
    }
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

fn verify(source: &str, expected: &str) -> io::Result<()> {
    if format!("{:x}", Sha256::digest(source.as_bytes())) != expected {
        return Err(invalid("Roblox assets do not match their metadata"));
    }

    Ok(())
}

fn interrupted(options: &Options, started: Instant) -> io::Result<()> {
    match options.interrupted(started) {
        Some(Reason::Cancelled) => Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Roblox asset download cancelled",
        )),

        Some(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Roblox asset download timed out",
        )),

        None => Ok(()),
    }
}

fn fetch(url: String, options: &Options) -> io::Result<BTreeMap<String, String>> {
    let started = Instant::now();
    interrupted(options, started)?;
    let (sender, receiver) = mpsc::sync_channel(1);
    let limits = options.clone();

    thread::Builder::new()
        .name("roblox".to_owned())
        .spawn(move || {
            drop(sender.send(download(&url, &limits)));
        })?;

    loop {
        interrupted(options, started)?;

        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => return result,
            Err(mpsc::RecvTimeoutError::Timeout) => {}

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("Roblox asset download stopped"));
            }
        }
    }
}

fn download(url: &str, options: &Options) -> io::Result<BTreeMap<String, String>> {
    let started = Instant::now();

    let agent = ureq::Agent::config_builder()
        .https_only(url.starts_with("https://"))
        .build()
        .new_agent();

    let get = |name: &str| -> io::Result<String> {
        interrupted(options, started)?;

        let bytes = agent
            .get(format!("{url}/{name}"))
            .config()
            .timeout_global(Some(options.timeout.saturating_sub(started.elapsed())))
            .build()
            .call()
            .map_err(io::Error::other)?
            .body_mut()
            .read_to_vec()
            .map_err(io::Error::other)?;

        String::from_utf8(bytes).map_err(invalid)
    };

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

        if name == "documentation.json"
            && !serde_json::from_str::<serde_json::Value>(&text)
                .map_err(invalid)?
                .is_object()
        {
            return Err(invalid("Roblox documentation must be an object"));
        }

        files.insert(name.to_owned(), text);
    }

    files.insert("metadata.json".to_owned(), source);

    Ok(files)
}

fn install(directory: &Path, mut files: BTreeMap<String, String>) -> io::Result<()> {
    let metadata = files
        .remove("metadata.json")
        .ok_or_else(|| invalid("missing Roblox metadata"))?;

    fs::create_dir_all(directory)?;

    let staging = directory.join(format!(
        ".download-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    fs::create_dir(&staging)?;

    let result = (|| {
        for (name, text) in files
            .into_iter()
            .chain([("metadata.json".to_owned(), metadata)])
        {
            let path = staging.join(&name);
            fs::write(&path, text)?;
            fs::rename(path, directory.join(name))?;
        }

        Ok(())
    })();

    let cleanup = fs::remove_dir_all(staging);

    result.and(cleanup)
}

#[cfg(test)]
#[path = "../tests/roblox/mod.rs"]
mod tests;
