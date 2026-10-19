use std::{collections::BTreeMap, fmt::Write as _};

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};

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

pub(crate) fn digest(contents: &[u8]) -> String {
    let mut digest = String::with_capacity(64);

    for byte in Sha256::digest(contents) {
        write!(digest, "{byte:02x}").expect("internal error: cannot write to a string");
    }

    digest
}
