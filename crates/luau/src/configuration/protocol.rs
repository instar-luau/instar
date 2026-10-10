use std::time::Duration;

use instar_shared::{Completion, Diagnostic, SourceVersion};
use serde::{Deserialize, Serialize};

use super::evaluation::ConfigurationInput;

#[derive(Deserialize, Serialize)]
pub(crate) struct ConfigurationRequest {
    pub request: u64,
    pub generation: u64,
    pub remaining_budget: Option<Duration>,
    pub input: ConfigurationInput,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Entry {
    pub key_kind: u8,
    pub key_text: String,
    pub key_number: f64,
    pub value_index: u32,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct Node {
    pub kind: u8,
    pub text: String,
    pub number: f64,
    pub boolean: bool,
    pub entries: Vec<Entry>,
}

#[derive(Deserialize, Serialize)]
pub(crate) struct ConfigurationResponse {
    pub request: u64,
    pub generation: u64,
    pub source: SourceVersion,
    pub root: Option<u32>,
    pub nodes: Vec<Node>,
    pub diagnostics: Vec<Diagnostic>,
    pub completion: Completion,
}
