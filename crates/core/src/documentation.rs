//! Namespaced documentation indexed by native symbol identifiers.

use std::{
    collections::{BTreeMap, btree_map::Entry as MapEntry},
    fmt, io,
    path::Path,
    time::Instant,
};

use instar_analysis::{Options, error::invalid};

use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};

use crate::project::Project;

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
    /// Looks up an exact native documentation identifier.
    #[must_use]
    pub fn get(&self, symbol: &str) -> Option<&Entry> {
        self.0.get(symbol)
    }

    pub(crate) fn parse(source: &str, namespaces: &[&str]) -> io::Result<Self> {
        let index: Self = serde_json::from_str(source).map_err(invalid)?;

        for (symbol, entry) in &index.0 {
            let (namespace, path) = symbol
                .split_once('/')
                .ok_or_else(|| invalid("documentation key requires a namespace and symbol path"))?;

            if !namespaces.contains(&namespace) || path.is_empty() || symbol.contains('\0') {
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

        Ok(index)
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

                Ok(Index(entries))
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
        let started = Instant::now();
        let source = crate::project::absolute(source)?;

        let directory = source
            .parent()
            .ok_or_else(|| invalid("source has no parent directory"))?;

        let settings = self.configuration(directory)?;

        let mut index = if settings.configuration.roblox.enabled == Some(true) {
            self.roblox_assets(settings.configuration.roblox.security, options, started)?
                .documentation
        } else {
            Index::default()
        };

        for environment in &settings.configuration.environment {
            for path in &environment.documentation {
                if let Some(reason) = options.interrupted(started) {
                    return Err(io::Error::new(
                        if reason == instar_analysis::Reason::Cancelled {
                            io::ErrorKind::Interrupted
                        } else {
                            io::ErrorKind::TimedOut
                        },
                        "documentation loading interrupted",
                    ));
                }

                let document = self.source(path)?;

                let loaded = Index::parse(&document.text, &[&environment.namespace])
                    .map_err(|error| invalid(format!("{}: {error}", path.display())))?;

                index.merge(loaded)?;
            }
        }

        Ok(index)
    }
}
