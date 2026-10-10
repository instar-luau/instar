use std::io;

use serde::{Deserialize, Serialize};

use crate::{SourceSnapshot, SourceVersion};

/// An original UTF-8 byte span with an exclusive end; empty cursor spans are valid.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct Span {
    /// Inclusive byte offset.
    pub start: usize,

    /// Exclusive byte offset, which may equal the source length.
    pub end: usize,
}

impl Span {
    /// Constructs a span validated against exact immutable source bytes.
    ///
    /// # Errors
    /// Returns invalid input for reversed, out-of-bounds or split-character spans.
    pub fn new(start: usize, end: usize, source: &SourceSnapshot) -> io::Result<Self> {
        let span = Self { start, end };
        span.validate(source)?;

        Ok(span)
    }

    /// Validates ordering, bounds and UTF-8 boundaries against this snapshot.
    ///
    /// # Errors
    /// Returns invalid input for invalid ranges and invalid data for inconsistent encoding.
    pub fn validate(self, source: &SourceSnapshot) -> io::Result<()> {
        source.validate()?;

        if self.start > self.end
            || self.end > source.text.len()
            || !source.text.is_char_boundary(self.start)
            || !source.text.is_char_boundary(self.end)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source span is not an ordered UTF-8 range within the snapshot",
            ));
        }

        Ok(())
    }
}

/// A byte span anchored to one exact source identity and revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct Location {
    /// Exact immutable input revision addressed by the span.
    pub source: SourceVersion,

    /// Original UTF-8 byte coordinates with an exclusive end.
    pub span: Span,
}

impl Location {
    /// Constructs a validated location at the supplied source revision.
    ///
    /// # Errors
    /// Returns invalid input for invalid ranges or invalid data for inconsistent encoding.
    pub fn new(source: &SourceSnapshot, span: Span) -> io::Result<Self> {
        span.validate(source)?;

        Ok(Self {
            source: source.version,
            span,
        })
    }

    /// Checks exact source agreement and validates the addressed byte span.
    ///
    /// # Errors
    /// Returns invalid input for source/revision mismatches or invalid spans.
    pub fn validate(self, source: &SourceSnapshot) -> io::Result<()> {
        if self.source != source.version {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "location does not address this source revision",
            ));
        }

        self.span.validate(source)?;

        Ok(())
    }
}
