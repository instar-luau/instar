use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metadata {
    pub(crate) services: Vec<String>,
    pub(crate) creatable_instances: Vec<String>,
    pub(crate) profiles: BTreeMap<String, String>,
    pub(crate) documentation: String,
    pub(crate) enumerations: BTreeMap<String, Vec<String>>,
    pub(crate) properties: BTreeMap<String, Vec<Access>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Documentation {
    pub(crate) documentation: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Access {
    pub(crate) class: String,
    pub(crate) name: String,
    pub(crate) read: bool,
    pub(crate) write: bool,
}
