//! Namespaced documentation indexed by native symbol identifiers.

use std::{
    collections::{BTreeMap, btree_map::Entry as MapEntry},
    fmt, io,
    path::Path,
};

use instar_shared::{Options, error::invalid};

use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, Visitor},
};

use crate::{assets, capture::Capture, configuration::validate_namespace};

/// Documentation attached to one native symbol.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentationEntry {
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
    pub params: Vec<DocumentationParameter>,

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
pub struct DocumentationParameter {
    /// Parameter name.
    pub name: String,

    /// Documentation identifier describing the parameter.
    pub documentation: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metadata {
    pub(crate) documentation: String,
    revision: String,
}

impl Metadata {
    pub(crate) fn parse(source: &str) -> io::Result<Self> {
        let metadata: Self = assets::parse(source)?;
        assets::revision(&metadata.revision)?;
        assets::digest(&metadata.documentation)?;

        Ok(metadata)
    }
}

/// A validated symbol-to-documentation index for an effective environment.
#[derive(Clone, Debug, Default)]
pub(crate) struct Index(BTreeMap<String, DocumentationEntry>);

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
    pub(crate) fn get(&self, symbol: &str) -> Option<&DocumentationEntry> {
        self.0.get(symbol)
    }

    fn validate(&self) -> io::Result<()> {
        for (symbol, entry) in &self.0 {
            let (namespace, path) = symbol
                .split_once('/')
                .ok_or_else(|| invalid("documentation key requires a namespace and symbol path"))?;

            validate_namespace(namespace)?;

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

                validate_namespace(namespace)?;

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

    pub(crate) fn merge(&mut self, other: Self) -> io::Result<()> {
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

                while let Some((symbol, entry)) = map.next_entry::<String, DocumentationEntry>()? {
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

pub(crate) fn load(
    directory: &Path,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<Index> {
    let Some(metadata_source) = capture.read(&directory.join("metadata.json"), options)? else {
        return Ok(Index::default());
    };

    let metadata = Metadata::parse(&metadata_source.text).map_err(|error| {
        assets::attributed(&metadata_source, &[], "documentation.metadata", error)
    })?;

    let documentation = capture.source(&directory.join("documentation.json"), options)?;
    options.check()?;

    assets::verify(&documentation.text, &metadata.documentation).map_err(|error| {
        assets::attributed(
            &documentation,
            &[metadata_source.version],
            "documentation.digest",
            error,
        )
    })?;

    let index = Index::parse(&documentation.text, &["@luau"])
        .map_err(|error| assets::attributed(&documentation, &[], "documentation.invalid", error))?;

    options.check()?;

    Ok(index)
}
