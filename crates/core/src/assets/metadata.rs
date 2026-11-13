use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};
use vermis::tree::NodeKind;

use super::{Error, mapping::invalid, syntax};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metadata {
    pub(crate) profiles: BTreeMap<String, Profile>,
    pub(crate) enumerations: Arc<BTreeMap<String, Vec<String>>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Profile {
    pub(crate) classes: Vec<Capability>,

    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deprecations: BTreeMap<String, BTreeMap<String, Deprecation>>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Deprecation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) replacement: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Capability {
    pub(crate) name: String,

    #[serde(rename = "service")]
    pub(crate) is_service: bool,

    #[serde(rename = "creatable")]
    pub(crate) is_creatable: bool,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) properties: Vec<Permission>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Permission {
    pub(crate) name: String,

    #[serde(rename = "read")]
    pub(crate) can_read: bool,

    #[serde(rename = "write")]
    pub(crate) can_write: bool,
}

impl Metadata {
    pub(super) fn validate(
        &self,
        definitions: &BTreeMap<String, impl AsRef<str>>,
    ) -> Result<(), Error> {
        if self.profiles.keys().ne(definitions.keys()) {
            return Err(invalid(
                "Roblox declarations and metadata have different security profiles.",
            ));
        }

        for (profile, source) in definitions {
            let tree = syntax::parse(source.as_ref())?;

            for node in &tree.nodes {
                if let NodeKind::TypeName {
                    namespace: Some(namespace),
                    name,
                    ..
                } = node.kind
                    && tree.text(namespace) == b"Enum"
                {
                    let name = syntax::text(&tree, name)?;

                    if !self.enumerations.contains_key(name) {
                        return Err(invalid(format!(
                            "Roblox `{profile}` declarations reference unknown enum `{name}`."
                        )));
                    }
                }
            }
        }

        Ok(())
    }
}

impl From<Profile> for instar_bridge::Environment {
    fn from(profile: Profile) -> Self {
        Self {
            classes: Some(
                profile
                    .classes
                    .into_iter()
                    .map(instar_bridge::Class::from)
                    .collect::<Vec<_>>()
                    .into(),
            ),
            deprecations: profile
                .deprecations
                .into_iter()
                .flat_map(|(owner, members)| {
                    members
                        .into_iter()
                        .map(move |(name, details)| instar_bridge::Deprecation {
                            owner: owner.clone(),
                            name,
                            replacement: details.replacement,
                        })
                })
                .collect::<Vec<_>>()
                .into(),
            ..Self::default()
        }
    }
}

impl From<Capability> for instar_bridge::Class {
    fn from(capability: Capability) -> Self {
        Self {
            name: capability.name,
            is_service: capability.is_service,
            is_creatable: capability.is_creatable,
            properties: capability
                .properties
                .into_iter()
                .map(|property| instar_bridge::Property {
                    name: property.name,
                    can_read: property.can_read,
                    can_write: property.can_write,
                })
                .collect(),
        }
    }
}
