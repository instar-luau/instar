//! Namespaced documentation indexed by native symbol identifiers.

use std::{
    collections::{BTreeMap, btree_map::Entry as MapEntry},
    fmt, io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::{Options, error::invalid};
use instar_bridge::frontend;

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};

use ureq::Agent;

use crate::{
    assets,
    project::{Project, absolute},
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

impl Index {
    pub(crate) fn parse(source: &str, namespaces: &[&str]) -> io::Result<Self> {
        let index: Self = assets::parse(source)?;

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

    /// Looks up an exact native documentation identifier.
    #[must_use]
    pub fn get(&self, symbol: &str) -> Option<&Entry> {
        self.0.get(symbol)
    }

    fn validate(&self) -> io::Result<()> {
        for (symbol, entry) in &self.0 {
            let (namespace, path) = symbol
                .split_once('/')
                .ok_or_else(|| invalid("documentation key requires a namespace and symbol path"))?;

            frontend::validate_namespace(namespace)?;

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

                frontend::validate_namespace(namespace)?;

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

impl<'source> Deserialize<'source> for Index {
    fn deserialize<Decoder: Deserializer<'source>>(
        deserializer: Decoder,
    ) -> Result<Self, Decoder::Error> {
        struct Symbols;

        impl<'source> Visitor<'source> for Symbols {
            type Value = Index;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object of unique documentation symbols")
            }

            fn visit_map<Access: MapAccess<'source>>(
                self,
                mut map: Access,
            ) -> Result<Index, Access::Error> {
                let mut entries = BTreeMap::new();

                while let Some((symbol, entry)) = map.next_entry::<String, Entry>()? {
                    match entries.entry(symbol) {
                        MapEntry::Vacant(slot) => {
                            slot.insert(entry);
                        }

                        MapEntry::Occupied(slot) => {
                            return Err(de::Error::custom(format!(
                                "duplicate documentation symbol: {}",
                                slot.key()
                            )));
                        }
                    }
                }

                let index = Index(entries);
                index.validate().map_err(de::Error::custom)?;

                Ok(index)
            }
        }

        deserializer.deserialize_map(Symbols)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    revision: String,
    documentation: String,
}

impl Project {
    /// Loads documentation for a source file's effective environment.
    ///
    /// # Errors
    /// Returns configuration, download, malformed-documentation, duplicate-symbol or interruption errors.
    pub fn documentation(&mut self, source: &Path, options: &Options) -> io::Result<Index> {
        self.during(
            |project| project.documentation_index(source, options),
            options,
        )
    }

    fn documentation_index(&mut self, source: &Path, options: &Options) -> io::Result<Index> {
        let started = Instant::now();
        let source = absolute(source)?;

        let directory = source
            .parent()
            .ok_or_else(|| invalid("source has no parent directory"))?;

        let settings = self.configuration(directory)?;

        let mut index = self.builtin_documentation(options, started)?;

        if settings.configuration.roblox.enabled == Some(true) {
            index.merge(
                self.roblox_assets(settings.configuration.roblox.security, options, started)?
                    .documentation
                    .clone(),
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
            Ok(_) => tracing::debug!(path = %directory.display(), "Using cached documentation"),

            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let files = assets::fetch(
                    format!("{}/luau", assets::HOST),
                    download,
                    &options.remaining(started),
                )?;

                assets::install(&directory, files, options, started)?;

                self.view.invalidate(&directory);
            }

            Err(error) => return Err(error),
        }

        let metadata = self.source(&directory.join("metadata.json"))?;
        let metadata: Metadata = assets::parse(&metadata.text)?;
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

    let agent = Agent::config_builder()
        .https_only(url.starts_with("https://"))
        .build()
        .new_agent();

    let metadata = assets::get(&agent, &format!("{url}/metadata.json"), options, started)?;
    let parsed: Metadata = assets::parse(&metadata)?;
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
