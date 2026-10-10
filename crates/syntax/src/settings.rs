use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Explicit lossless-parser language context.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct SyntaxSettings {
    /// Additional feature profiles; unknown profiles are unavailable.
    pub features: Vec<String>,

    /// Whether declaration syntax is accepted.
    pub declaration_syntax: bool,

    /// Whether the UTF-8 byte-order marker is accepted.
    pub encoding_marker: bool,
}

impl Default for SyntaxSettings {
    fn default() -> Self {
        Self {
            features: Vec::new(),
            declaration_syntax: true,
            encoding_marker: true,
        }
    }
}
