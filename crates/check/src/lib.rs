//! Luau type checking.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// File selection for checking; native language settings belong to Luau configuration.
#[derive(Clone, Debug, Default, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct Configuration {
    /// Include patterns; omission inherits and an empty list selects every source.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub include: Option<Vec<String>>,

    /// Exclude patterns; omission inherits and an empty list clears exclusions.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Vec<String>")]
    pub exclude: Option<Vec<String>>,
}
