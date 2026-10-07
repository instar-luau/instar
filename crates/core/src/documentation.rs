//! Namespaced documentation indexed by native symbol identifiers.

use std::{
    collections::{BTreeMap, btree_map::Entry as MapEntry},
    fmt, io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Options, error::invalid};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};

use crate::{
    assets,
    project::{Change, Project},
};

/// Documentation attached to one native symbol.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Markdown or HTML documentation text.
    pub documentation: Option<String>,

    /// External reference URL.
    pub learn_more_link: Option<String>,

    /// Example source text.
    pub code_sample: Option<String>,

    /// Member names mapped to documentation identifiers.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,

    /// Documented function parameters.
    #[serde(default)]
    pub params: Vec<Parameter>,

    /// Return-value documentation identifiers.
    #[serde(default)]
    pub returns: Vec<String>,

    /// Overload signatures mapped to documentation identifiers.
    #[serde(default)]
    pub overloads: BTreeMap<String, String>,
}

/// A named parameter and its documentation reference.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameter {
    /// Parameter name.
    pub name: String,

    /// Documentation identifier describing the parameter.
    pub documentation: String,
}

/// A validated symbol-to-documentation index for an effective environment.
#[derive(Clone, Debug, Default)]
pub struct Index(BTreeMap<String, Entry>);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    revision: String,
    documentation: String,
}

impl Index {
    /// Looks up an exact native documentation identifier.
    #[must_use]
    pub fn get(&self, symbol: &str) -> Option<&Entry> {
        self.0.get(symbol)
    }

    pub(crate) fn parse(source: &str, namespaces: &[&str]) -> io::Result<Self> {
        let index: Self = serde_json::from_str(source).map_err(invalid)?;

        for symbol in index.0.keys() {
            if !symbol
                .split_once('/')
                .is_some_and(|(namespace, _)| namespaces.contains(&namespace))
            {
                return Err(invalid(format!(
                    "documentation namespace mismatch: {symbol}"
                )));
            }
        }

        Ok(index)
    }

    fn validate(&self) -> io::Result<()> {
        for (symbol, entry) in &self.0 {
            let (namespace, path) = symbol
                .split_once('/')
                .ok_or_else(|| invalid("documentation key requires a namespace and symbol path"))?;

            instar_bridge::frontend::validate_namespace(namespace)?;

            if path.is_empty() || symbol.contains('\0') {
                return Err(invalid(format!(
                    "documentation namespace mismatch: {symbol}"
                )));
            }

            if entry.documentation.is_none() && entry.overloads.is_empty() {
                return Err(invalid(format!(
                    "documentation entry has no content: {symbol}"
                )));
            }

            for reference in entry
                .keys
                .values()
                .chain(entry.overloads.values())
                .chain(&entry.returns)
                .chain(
                    entry
                        .params
                        .iter()
                        .map(|parameter| &parameter.documentation),
                )
            {
                let (namespace, path) = reference
                    .split_once('/')
                    .ok_or_else(|| invalid("invalid documentation reference"))?;

                instar_bridge::frontend::validate_namespace(namespace)?;

                if path.is_empty() || reference.contains('\0') {
                    return Err(invalid("invalid documentation reference"));
                }
            }

            if entry
                .params
                .iter()
                .any(|parameter| parameter.name.is_empty() || parameter.name.contains('\0'))
            {
                return Err(invalid("invalid documentation parameter name"));
            }
        }

        Ok(())
    }

    fn merge(&mut self, other: Self) -> io::Result<()> {
        for (symbol, entry) in other.0 {
            match self.0.entry(symbol) {
                MapEntry::Vacant(slot) => {
                    slot.insert(entry);
                }

                MapEntry::Occupied(slot) => {
                    return Err(invalid(format!(
                        "duplicate documentation symbol: {}",
                        slot.key()
                    )));
                }
            }
        }

        Ok(())
    }
}

impl<'de> Deserialize<'de> for Index {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Symbols;

        impl<'de> Visitor<'de> for Symbols {
            type Value = Index;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of unique documentation symbols")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Index, A::Error> {
                let mut entries = BTreeMap::new();

                while let Some((symbol, entry)) = map.next_entry::<String, Entry>()? {
                    match entries.entry(symbol) {
                        MapEntry::Vacant(slot) => {
                            slot.insert(entry);
                        }

                        MapEntry::Occupied(slot) => {
                            return Err(serde::de::Error::custom(format!(
                                "duplicate documentation symbol: {}",
                                slot.key()
                            )));
                        }
                    }
                }

                let index = Index(entries);
                index.validate().map_err(serde::de::Error::custom)?;

                Ok(index)
            }
        }

        deserializer.deserialize_map(Symbols)
    }
}

impl Project {
    /// Loads documentation for a source file's effective environment.
    ///
    /// # Errors
    /// Returns configuration, download, malformed-documentation, duplicate-symbol or interruption errors.
    pub fn documentation(&mut self, source: &Path, options: &Options) -> io::Result<Index> {
        self.during(options, |project| {
            project.documentation_index(source, options)
        })
    }

    fn documentation_index(&mut self, source: &Path, options: &Options) -> io::Result<Index> {
        let started = Instant::now();
        let source = crate::project::absolute(source)?;

        let directory = source
            .parent()
            .ok_or_else(|| invalid("source has no parent directory"))?;

        let settings = self.configuration(directory)?;

        let mut index = self.builtin_documentation(options, started)?;

        if settings.configuration.roblox.enabled == Some(true) {
            index.merge(
                self.roblox_assets(settings.configuration.roblox.security, options, started)?
                    .documentation,
            )?;
        }

        for environment in &settings.configuration.environment {
            for path in &environment.documentation {
                options.check(started)?;

                let document = self.source(path)?;

                let loaded = Index::parse(&document.text, &[&environment.namespace])
                    .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

                index.merge(loaded)?;
            }
        }

        options.check(started)?;

        Ok(index)
    }

    fn builtin_documentation(&mut self, options: &Options, started: Instant) -> io::Result<Index> {
        let directory = cache_directory()?;

        match self.source(&directory.join("metadata.json")) {
            Ok(_) => {}

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let files = assets::fetch(
                    format!("{}/luau/{}", assets::HOST, assets::VERSION),
                    &options.remaining(started),
                    download,
                )?;

                options.check(started)?;
                assets::install(&directory, files)?;

                self.view
                    .change(&directory, Change::Disk(directory.clone()));
            }

            Err(error) => return Err(error),
        }

        let metadata = self.source(&directory.join("metadata.json"))?;
        let metadata: Metadata = serde_json::from_str(&metadata.text).map_err(invalid)?;
        assets::revision(&metadata.revision)?;
        let document = self.source(&directory.join("documentation.json"))?;
        assets::verify(&document.text, &metadata.documentation)?;

        Index::parse(&document.text, &["@luau"])
    }
}

/// Returns the versioned cache for standalone Luau documentation.
///
/// # Errors
/// Returns an error when the operating system's user cache directory is unavailable.
pub fn cache_directory() -> io::Result<PathBuf> {
    assets::directory("luau")
}

fn download(url: &str, options: &Options) -> io::Result<BTreeMap<String, String>> {
    let started = Instant::now();

    let agent = ureq::Agent::config_builder()
        .https_only(url.starts_with("https://"))
        .build()
        .new_agent();

    let metadata = assets::get(&agent, &format!("{url}/metadata.json"), options, started)?;
    let parsed: Metadata = serde_json::from_str(&metadata).map_err(invalid)?;
    assets::revision(&parsed.revision)?;

    let documentation = assets::get(
        &agent,
        &format!("{url}/documentation.json"),
        options,
        started,
    )?;

    assets::verify(&documentation, &parsed.documentation)?;
    Index::parse(&documentation, &["@luau"])?;

    Ok(BTreeMap::from([
        ("metadata.json".to_owned(), metadata),
        ("documentation.json".to_owned(), documentation),
    ]))
}
