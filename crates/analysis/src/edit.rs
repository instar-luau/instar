use serde::{Deserialize, Serialize};

use crate::Location;

/// A source replacement anchored to an immutable host module revision.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Edit<Module> {
    /// Exact source identity, revision, and byte range to replace.
    pub location: Location<Module>,

    /// UTF-8 replacement text.
    pub text: String,
}
