use std::{collections::BTreeMap, sync::Arc};

use serde::{Deserialize, Serialize};

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
