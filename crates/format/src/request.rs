use std::io;

use instar_shared::{SourceSnapshot, Span};
use instar_syntax::SyntaxSettings;
use serde::{Deserialize, Serialize};

use crate::FormatSettings;

/// Complete immutable syntax-only formatting inputs.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FormatInput {
    /// Original source identity, revision and bytes.
    pub source: SourceSnapshot,

    /// Effective layout and requested syntax transformations.
    pub settings: FormatSettings,

    /// Selected lossless grammar context.
    pub syntax: SyntaxSettings,

    /// Requested source selections.
    pub scope: FormatScope,
}

impl FormatInput {
    /// Validates exact source, settings and selection boundaries.
    ///
    /// # Errors
    /// Returns invalid source encoding, configuration or ranges.
    pub fn validate(&self) -> io::Result<()> {
        self.source.validate()?;
        self.settings.validate()?;

        for span in self.requested()? {
            span.validate(&self.source)?;
        }

        Ok(())
    }

    pub(crate) fn requested(&self) -> io::Result<Vec<Span>> {
        match &self.scope {
            FormatScope::Whole => Ok(vec![Span {
                start: 0,
                end: self.source.text.len(),
            }]),

            FormatScope::Ranges(ranges) => Ok(ranges.clone()),

            FormatScope::OnType { offset, trigger } => {
                Span {
                    start: *offset,
                    end: *offset,
                }
                .validate(&self.source)?;

                let start = if *trigger == '\n' {
                    self.source.text[..*offset]
                        .trim_end_matches(['\r', '\n'])
                        .rfind('\n')
                        .map_or(0, |position| position + 1)
                } else {
                    *offset
                };

                Ok(vec![Span {
                    start,
                    end: *offset,
                }])
            }
        }
    }
}

/// Whole-source, multiple-range or caller-triggered formatting.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum FormatScope {
    /// Format a complete syntactically valid source.
    Whole,

    /// Expand and merge ranges against the original snapshot.
    Ranges(Vec<Span>),

    /// Format the enclosing syntax after a caller-selected trigger.
    OnType {
        /// Original UTF-8 byte cursor offset.
        offset: usize,
        /// Caller-selected formatting trigger.
        trigger: char,
    },
}
