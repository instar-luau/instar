use std::{collections::BTreeMap, io};

use serde::{Deserialize, Serialize};

use crate::{Snapshot, flags};

/// One host-extracted require site, anchored to immutable source bytes.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Site {
    /// Entire call byte range, with exclusive end.
    pub call: [usize; 2],

    /// Argument byte range, with exclusive end.
    pub argument: [usize; 2],

    /// Whether the host statically identified this request, including failures.
    pub static_request: bool,

    /// Host-resolved target module name; `None` retains an unresolved site.
    pub target: Option<String>,
}

/// An agreed native graph edge backed by a host require site.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Link {
    /// Opaque requiring module name.
    pub module: String,

    /// Source revision anchoring the site.
    pub revision: u64,

    /// Entire call byte range.
    pub call: [usize; 2],

    /// Require argument byte range.
    pub argument: [usize; 2],

    /// Opaque host-resolved target name.
    pub target: String,
}

/// Immutable declaration file used to establish a module's native environment.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Definition {
    /// Opaque host declaration identity.
    pub name: String,

    /// Documentation namespace independent of the declaration's source identity.
    pub namespace: String,

    /// Source revision.
    pub revision: u64,

    /// Declaration source bytes.
    pub text: String,
}

/// A Roblox class capability supplied by the asset metadata.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Class {
    /// Declared class name.
    pub name: String,

    /// Whether `GetService` accepts the class.
    pub service: bool,

    /// Whether Instance.new accepts the class.
    pub creatable: bool,

    /// Property access rules that require native application.
    pub properties: Vec<Property>,
}

/// Independent read and write access to a declared Roblox property.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Property {
    /// Exact property name.
    pub name: String,

    /// Whether reading is permitted.
    pub read: bool,

    /// Whether writing is permitted.
    pub write: bool,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Source {
    pub(crate) configuration: Snapshot,
    pub(crate) text: String,
    pub(crate) revision: u64,
    pub(crate) sites: Vec<Site>,
    pub(crate) definitions: Vec<Definition>,
    pub(crate) classes: Vec<Class>,
    pub(crate) flags: BTreeMap<String, flags::Value>,
}

/// Validates the namespace component of a native documentation identifier.
///
/// # Errors
/// Rejects empty namespaces and characters outside ASCII letters, digits, underscores and hyphens.
pub fn validate_namespace(namespace: &str) -> io::Result<()> {
    let valid = namespace.strip_prefix('@').is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });

    if !valid {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid documentation namespace",
        ));
    }

    Ok(())
}
