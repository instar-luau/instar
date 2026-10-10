use std::{io, sync::Arc};

use instar_shared::{Completion, Diagnostic, Reason, SourceVersion, Span};
use instar_syntax::PreservationResult;
use serde::{Deserialize, Serialize};

use crate::FormatInput;

/// Formatting outcome attributed to the exact source revision.
#[derive(Clone, Debug)]
pub struct FormatResult {
    /// Original source revision.
    pub source: SourceVersion,

    /// Applicable validated proposal or explicit unavailability without output.
    pub outcome: FormatOutcome,

    /// Parser, directive or preservation diagnostics.
    pub diagnostics: Vec<Diagnostic>,

    /// Terminal operation completion.
    pub completion: Completion,
}

impl FormatResult {
    pub(crate) fn unavailable(
        input: &FormatInput,
        reason: Reason,
        selections: Vec<FormatSelection>,
        preservation: Option<PreservationResult>,
        diagnostics: Vec<Diagnostic>,
        completion: Completion,
    ) -> Self {
        Self {
            source: input.source.version,
            outcome: FormatOutcome::Unavailable {
                reason,
                selections,
                preservation,
            },
            diagnostics,
            completion,
        }
    }

    pub(crate) fn interrupted(input: &FormatInput, completion: Completion) -> io::Result<Self> {
        let selections = input
            .requested()?
            .into_iter()
            .map(|requested| FormatSelection {
                requested,
                expanded: Vec::new(),
                outcome: SelectionOutcome::Unavailable(Reason::FailedPrerequisite),
            })
            .collect();

        Ok(Self::unavailable(
            input,
            Reason::FailedPrerequisite,
            selections,
            None,
            Vec::new(),
            completion,
        ))
    }
}

/// A complete validated proposal or unavailable formatting evidence.
#[derive(Clone, Debug)]
pub enum FormatOutcome {
    /// Every requested formatting and preservation guarantee completed.
    Validated(ValidatedFormat),

    /// Unsuccessful formatting never exposes replacement source.
    Unavailable {
        /// Required guarantee or prerequisite that was unavailable.
        reason: Reason,
        /// Per-request selection evidence.
        selections: Vec<FormatSelection>,
        /// Available preservation evidence.
        preservation: Option<PreservationResult>,
    },
}

/// An opaque proposal reconstructed only after exact host validation.
#[derive(Clone, Debug)]
pub struct ValidatedFormat {
    input: FormatInput,
    output: Arc<str>,
    selections: Vec<FormatSelection>,
    preservation: PreservationResult,
}

impl ValidatedFormat {
    /// Returns the complete original inputs.
    #[must_use]
    pub fn input(&self) -> &FormatInput {
        &self.input
    }

    /// Returns the complete validated replacement source.
    #[must_use]
    pub fn output(&self) -> &str {
        &self.output
    }

    /// Returns an outcome for every requested selection.
    #[must_use]
    pub fn selections(&self) -> &[FormatSelection] {
        &self.selections
    }

    /// Returns completed syntax and byte-preservation evidence.
    #[must_use]
    pub fn preservation(&self) -> &PreservationResult {
        &self.preservation
    }

    pub(crate) fn new(
        input: FormatInput,
        output: Arc<str>,
        selections: Vec<FormatSelection>,
        preservation: PreservationResult,
    ) -> Self {
        Self {
            input,
            output,
            selections,
            preservation,
        }
    }
}

/// One requested range and its safe expansion.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct FormatSelection {
    /// Caller-requested original bytes.
    pub requested: Span,

    /// Safely expanded original syntax spans.
    pub expanded: Vec<Span>,

    /// Whether this selection changed or was unavailable.
    pub outcome: SelectionOutcome,
}

/// Per-selection outcome.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum SelectionOutcome {
    /// Formatting completed without changing the selected bytes.
    Unchanged,

    /// Formatting changed the selected bytes.
    Formatted,

    /// Required syntax or preservation coverage was unavailable.
    Unavailable(Reason),
}
