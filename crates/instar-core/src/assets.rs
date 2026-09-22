use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use instar_bridge::{
    Callbacks, Checker, CheckerOptions, Configuration, ResolveRequest, RobloxClass, Source,
};

use reqwest::{
    StatusCode, Url,
    blocking::Client,
    header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED},
};

use serde::{Deserialize, Serialize};

use crate::{absolute, invalid};

pub(crate) const BASE: &str = "https://instar-luau.github.io/instar/";
const LIMIT: u64 = 32 * 1024 * 1024;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    services: Vec<String>,
    creatable_instances: Vec<String>,
}

pub(crate) struct Definitions {
    pub(crate) source: Rc<str>,
    metadata: Option<Metadata>,
}

impl Definitions {
    fn combine(sources: &[Rc<str>]) -> io::Result<Self> {
        let mut source = String::with_capacity(sources.iter().map(|source| source.len() + 1).sum());
        let mut combined: Option<Metadata> = None;

        for text in sources {
            if let Some(metadata) = parse_metadata(text)? {
                let combined = combined.get_or_insert_default();
                combined.services.extend(metadata.services);

                combined
                    .creatable_instances
                    .extend(metadata.creatable_instances);
            }

            source.push_str(text);
            source.push('\n');
        }

        Ok(Self {
            source: source.into(),
            metadata: combined,
        })
    }

    pub(crate) fn register(&self, checker: &mut Checker) -> io::Result<bool> {
        let Some(metadata) = &self.metadata else {
            return Ok(false);
        };

        let mut classes = BTreeMap::new();

        for name in &metadata.services {
            classes.insert(
                name.as_str(),
                RobloxClass {
                    name,
                    service: true,
                    creatable: false,
                },
            );
        }

        for name in &metadata.creatable_instances {
            classes
                .entry(name.as_str())
                .or_insert(RobloxClass {
                    name,
                    service: false,
                    creatable: false,
                })
                .creatable = true;
        }

        checker.register_roblox_classes(&classes.into_values().collect::<Vec<_>>())?;

        Ok(true)
    }
}

fn parse_metadata(source: &str) -> io::Result<Option<Metadata>> {
    let Some(header) = source
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("--#METADATA#"))
    else {
        return Ok(None);
    };

    let metadata: Metadata = serde_json::from_str(header)?;

    for names in [&metadata.services, &metadata.creatable_instances] {
        let mut seen = HashSet::new();

        for name in names {
            if name.is_empty() || name.contains('\0') || !seen.insert(name) {
                return Err(invalid(
                    "Roblox metadata contains an empty, invalid, or duplicate class name",
                ));
            }
        }
    }

    Ok(Some(metadata))
}

fn validate_definition_file(source: &str) -> io::Result<()> {
    parse_metadata(source).map(|_| ())
}

struct DefinitionHost {
    configuration: Configuration,
    errors: Vec<String>,
}

impl Callbacks for DefinitionHost {
    fn source<'source>(&'source mut self, name: &str) -> io::Result<Source<'source>> {
        Err(invalid(format!("declarations requested source {name}")))
    }

    fn configuration(&mut self, _name: &str) -> io::Result<&Configuration> {
        Ok(&self.configuration)
    }

    fn resolve(&mut self, _request: &ResolveRequest<'_>) -> io::Result<Option<String>> {
        Err(invalid("declarations cannot import source modules"))
    }

    fn diagnostic(&mut self, diagnostic: instar_bridge::Diagnostic<'_>) -> io::Result<()> {
        self.errors.push(diagnostic.message.to_owned());

        Ok(())
    }
}

fn validate_definitions(definitions: &Definitions) -> io::Result<()> {
    let mut checker = Checker::new(&CheckerOptions::default())?;

    let mut host = DefinitionHost {
        configuration: Configuration::new(b"{}")?,
        errors: Vec::new(),
    };

    checker
        .load_definition(&mut host, definitions.source.as_bytes(), "roblox")
        .map_err(|error| invalid(format!("{error}: {}", host.errors.join("; "))))?;

    if !host.errors.is_empty() {
        return Err(invalid(host.errors.join("; ")));
    }

    definitions.register(&mut checker).map(|_| ())
}

fn validate_documentation(source: &str) -> io::Result<()> {
    let value: serde_json::Value = serde_json::from_str(source)?;

    if !value.is_object() {
        return Err(invalid("Roblox documentation must be a JSON object"));
    }

    Ok(())
}

pub(crate) fn location(value: &str, directory: &Path) -> io::Result<String> {
    if value.trim().is_empty() || value.contains('\0') {
        return Err(invalid(
            "asset location must be nonempty and contain no NUL",
        ));
    }

    if value.contains("://") {
        let url = Url::parse(value).map_err(|error| invalid(error.to_string()))?;

        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(invalid(
                "asset URLs must use HTTPS without credentials or a fragment",
            ));
        }

        Ok(url.into())
    } else {
        absolute(&directory.join(value))?
            .to_str()
            .map(str::to_owned)
            .ok_or_else(|| invalid("asset path requires UTF-8"))
    }
}

#[derive(Deserialize, Serialize)]
struct Cached {
    url: String,
    etag: Option<String>,
    modified: Option<String>,
    body: String,
}

#[derive(Default)]
pub(crate) struct Assets {
    client: Option<Client>,
    loaded: HashMap<String, Result<Rc<str>, String>>,
    definitions: HashMap<Vec<String>, Rc<Definitions>>,
    documentation: HashMap<Vec<String>, Rc<serde_json::Value>>,
    pending: HashMap<String, Cached>,
    previous: HashMap<String, Rc<str>>,
    warnings: Vec<String>,
}

impl Assets {
    pub(crate) fn definitions(&mut self, locations: &[String]) -> io::Result<Rc<Definitions>> {
        if let Some(definitions) = self.definitions.get(locations) {
            return Ok(Rc::clone(definitions));
        }

        let mut sources = Vec::new();
        let mut unique = Vec::new();

        for location in locations {
            if unique.contains(&location) {
                continue;
            }

            sources.push(self.load(location, validate_definition_file, false)?);
            unique.push(location);
        }

        let mut definitions = Definitions::combine(&sources)?;

        if let Err(error) = validate_definitions(&definitions) {
            let mut restored = false;

            for (location, source) in unique.iter().zip(&mut sources) {
                if self.pending.contains_key(*location) {
                    let Some(previous) = self.previous.get(*location) else {
                        return Err(error);
                    };

                    *source = Rc::clone(previous);
                    restored = true;
                }
            }

            if !restored {
                return Err(error);
            }

            let fallback = Definitions::combine(&sources)?;

            validate_definitions(&fallback)
                .map_err(|_| error.to_string())
                .map_err(invalid)?;

            for (location, source) in unique.iter().zip(&sources) {
                if self.pending.remove(*location).is_some() {
                    self.warnings.push(format!(
                        "Using cached {location}; declaration update failed: {error}"
                    ));

                    self.loaded
                        .insert((*location).clone(), Ok(Rc::clone(source)));
                }
            }

            definitions = fallback;
        }

        for location in unique {
            if let Some(cached) = self.pending.remove(location)
                && let Err(error) = cache_path(location).and_then(|path| save_cache(&path, &cached))
            {
                self.warnings
                    .push(format!("Could not cache {location}: {error}"));
            }
        }

        let definitions = Rc::new(definitions);

        self.definitions
            .insert(locations.to_vec(), Rc::clone(&definitions));

        Ok(definitions)
    }

    pub(crate) fn documentation(
        &mut self,
        locations: &[String],
    ) -> io::Result<Rc<serde_json::Value>> {
        if let Some(documentation) = self.documentation.get(locations) {
            return Ok(Rc::clone(documentation));
        }

        let mut merged = serde_json::Map::new();

        for location in locations {
            let body = self.load(location, validate_documentation, true)?;
            let entries: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&body)?;
            merged.extend(entries);
        }

        let documentation = Rc::new(serde_json::Value::Object(merged));

        self.documentation
            .insert(locations.to_vec(), Rc::clone(&documentation));

        Ok(documentation)
    }

    pub(crate) fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }

    fn load(
        &mut self,
        location: &str,
        validate: fn(&str) -> io::Result<()>,
        persist: bool,
    ) -> io::Result<Rc<str>> {
        if let Some(result) = self.loaded.get(location) {
            return result.clone().map_err(invalid);
        }

        let result = self
            .load_uncached(location, validate, persist)
            .map(Rc::<str>::from)
            .map_err(|error| format!("{location}: {error}"));

        self.loaded.insert(location.to_owned(), result.clone());

        result.map_err(invalid)
    }

    fn load_uncached(
        &mut self,
        location: &str,
        validate: fn(&str) -> io::Result<()>,
        persist: bool,
    ) -> io::Result<String> {
        if !location.starts_with("https://") {
            let body = read_limited(fs::File::open(location)?, LIMIT)?;
            validate(&body)?;

            return Ok(body);
        }

        let path = cache_path(location)?;

        let cached = match fs::File::open(&path) {
            Ok(file) => read_limited(file, LIMIT * 6)
                .and_then(|body| serde_json::from_str::<Cached>(&body).map_err(io::Error::other))
                .and_then(|cached| {
                    if cached.url != location {
                        return Err(invalid("cache URL mismatch"));
                    }

                    validate(&cached.body)?;

                    Ok(cached)
                })
                .map(Some),

            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        };

        let cached = match cached {
            Ok(cached) => cached,

            Err(error) => {
                self.warnings.push(format!(
                    "Ignoring unusable Roblox cache {}: {error}",
                    path.display()
                ));

                None
            }
        };

        if !persist && let Some(cached) = &cached {
            self.previous
                .insert(location.to_owned(), Rc::from(cached.body.as_str()));
        }

        match self.download(location, cached.as_ref()) {
            Ok(None) => Ok(cached
                .ok_or_else(|| invalid("server returned 304 without a cached asset"))?
                .body),

            Ok(Some(updated)) => {
                if let Err(error) = validate(&updated.body) {
                    return self.fallback(location, cached, &error);
                }

                if persist {
                    if let Err(error) = save_cache(&path, &updated) {
                        self.warnings
                            .push(format!("Could not cache {location}: {error}"));
                    }
                } else {
                    let body = updated.body.clone();
                    self.pending.insert(location.to_owned(), updated);

                    return Ok(body);
                }

                Ok(updated.body)
            }

            Err(error) => self.fallback(location, cached, &error),
        }
    }

    fn fallback(
        &mut self,
        location: &str,
        cached: Option<Cached>,
        error: &io::Error,
    ) -> io::Result<String> {
        if let Some(cached) = cached {
            self.warnings
                .push(format!("Using cached {location}; refresh failed: {error}"));

            Ok(cached.body)
        } else {
            Err(invalid(format!(
                "could not load asset and no usable cache exists: {error}"
            )))
        }
    }

    fn download(&mut self, location: &str, cached: Option<&Cached>) -> io::Result<Option<Cached>> {
        if self.client.is_none() {
            self.client = Some(
                Client::builder()
                    .https_only(true)
                    .timeout(Duration::from_secs(30))
                    .connect_timeout(Duration::from_secs(10))
                    .user_agent(concat!("instar/", env!("CARGO_PKG_VERSION")))
                    .build()
                    .map_err(io::Error::other)?,
            );
        }

        let mut request = self
            .client
            .as_ref()
            .expect("client initialized")
            .get(location);

        if let Some(cached) = cached {
            if let Some(etag) = &cached.etag {
                request = request.header(IF_NONE_MATCH, etag);
            } else if let Some(modified) = &cached.modified {
                request = request.header(IF_MODIFIED_SINCE, modified);
            }
        }

        let response = request.send().map_err(io::Error::other)?;

        if response.status() == StatusCode::NOT_MODIFIED {
            return Ok(None);
        }

        let response = response.error_for_status().map_err(io::Error::other)?;

        if response
            .content_length()
            .is_some_and(|length| length > LIMIT)
        {
            return Err(invalid("Roblox asset exceeds 32 MiB"));
        }

        let etag = response
            .headers()
            .get(ETAG)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let modified = response
            .headers()
            .get(LAST_MODIFIED)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        Ok(Some(Cached {
            url: location.to_owned(),
            etag,
            modified,
            body: read_limited(response, LIMIT)?,
        }))
    }
}

fn read_limited(reader: impl Read, limit: u64) -> io::Result<String> {
    let mut bytes = Vec::new();
    reader.take(limit + 1).read_to_end(&mut bytes)?;

    if u64::try_from(bytes.len()).map_err(io::Error::other)? > limit {
        return Err(invalid("Roblox asset exceeds size limit"));
    }

    String::from_utf8(bytes).map_err(|error| invalid(error.to_string()))
}

fn cache_path(url: &str) -> io::Result<PathBuf> {
    let directory = if cfg!(target_os = "windows") {
        env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .filter(|path| path.is_absolute())
    .ok_or_else(|| invalid("cannot determine the per-user cache directory"))?;

    let mut hash = DefaultHasher::new();
    url.hash(&mut hash);

    Ok(directory
        .join("instar/roblox")
        .join(format!("{:016x}.json", hash.finish())))
}

fn save_cache(path: &Path, cached: &Cached) -> io::Result<()> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| invalid("cache has no parent directory"))?,
    )?;

    let temporary = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;

    let result = (|| {
        serde_json::to_writer(&mut file, cached)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        drop(file);

        fs::rename(&temporary, path)
    })();

    if result.is_err() {
        fs::remove_file(&temporary).ok();
    }

    result
}
