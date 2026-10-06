use instar_analysis::{Completion, Reason};
use serde::{Deserialize, Serialize};

use crate::Diagnostic;

/// Formatting output and explicit operation completeness.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Result {
    /// Complete formatted source, available only when formatting succeeds.
    pub output: Option<Vec<u8>>,

    /// Source syntax or formatting analysis failures.
    pub diagnostics: Vec<Diagnostic>,

    /// Whether the requested operation finished.
    pub completion: Completion,
}

impl Result {
    pub(crate) fn interrupted(reason: Reason) -> Self {
        Self {
            output: None,
            diagnostics: Vec::new(),
            completion: Completion::Incomplete(reason),
        }
    }

    pub(crate) fn failed(message: &str) -> Self {
        Self {
            output: None,
            diagnostics: vec![Diagnostic {
                range: [0, 0],
                message: message.to_owned(),
            }],
            completion: Completion::Incomplete(Reason::Analysis),
        }
    }
}
