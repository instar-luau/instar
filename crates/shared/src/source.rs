use std::{io, str, sync::Arc};

use serde::{Deserialize, Deserializer, Serialize, de::Error as _};

use crate::{Span, error::invalid};

/// An opaque source identity allocated by its owning store, independent of paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct SourceIdentity(u64);

impl SourceIdentity {
    /// Wraps a value allocated uniquely by the owning source store.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the store's identity value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// An opaque project context identity allocated by its owning store.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct ContextIdentity(u64);

impl ContextIdentity {
    /// Wraps a value allocated uniquely by the owning context store.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the store's identity value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// The exact immutable revision of a source input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub struct SourceVersion {
    /// Store-owned identity; provenance does not grant write permission.
    pub identity: SourceIdentity,

    /// Revision assigned by the owning source store.
    pub revision: u64,
}

/// The supported UTF-8 encoding and original byte-order marker state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum SourceEncoding {
    /// UTF-8 without a leading byte-order marker.
    Utf8,

    /// UTF-8 with the original three-byte marker retained in the text.
    Utf8Marker,
}

impl SourceEncoding {
    /// Returns the byte prefix a parser must account for in source coordinates.
    #[must_use]
    pub const fn prefix(self) -> usize {
        match self {
            Self::Utf8 => 0,
            Self::Utf8Marker => 3,
        }
    }
}

/// Immutable original UTF-8 source bytes and their exact identity and revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceSnapshot {
    /// Exact input revision.
    pub version: SourceVersion,

    /// Original text, including a supported byte-order marker.
    pub text: Arc<str>,

    /// Original encoding marker state.
    pub encoding: SourceEncoding,
}

impl SourceSnapshot {
    /// Captures original UTF-8 text without normalizing its bytes.
    ///
    /// # Errors
    /// Returns invalid data for an unsupported encoding marker.
    pub fn new(version: SourceVersion, text: impl Into<Arc<str>>) -> io::Result<Self> {
        let text = text.into();
        reject_encoding(text.as_bytes())?;

        let encoding = if text.starts_with('\u{feff}') {
            SourceEncoding::Utf8Marker
        } else {
            SourceEncoding::Utf8
        };

        Ok(Self {
            version,
            text,
            encoding,
        })
    }

    /// Decodes original source bytes as UTF-8, retaining their marker.
    ///
    /// # Errors
    /// Returns invalid data for unsupported encodings or malformed UTF-8.
    pub fn from_bytes(version: SourceVersion, bytes: &[u8]) -> io::Result<Self> {
        reject_encoding(bytes)?;
        let text = str::from_utf8(bytes).map_err(invalid)?;

        Self::new(version, text)
    }

    /// Validates externally constructed or deserialized snapshot encoding metadata.
    ///
    /// # Errors
    /// Returns invalid data for an unsupported or inconsistent encoding marker.
    pub fn validate(&self) -> io::Result<()> {
        reject_encoding(self.text.as_bytes())?;

        if self.text.starts_with('\u{feff}') != (self.encoding == SourceEncoding::Utf8Marker) {
            return Err(invalid(
                "source encoding does not match its byte-order marker",
            ));
        }

        Ok(())
    }

    /// Builds an owned line index retaining this exact immutable text.
    #[must_use]
    pub fn line_index(&self) -> LineIndex {
        LineIndex::new(Arc::clone(&self.text))
    }
}

impl<'input> Deserialize<'input> for SourceSnapshot {
    fn deserialize<Input>(deserializer: Input) -> Result<Self, Input::Error>
    where
        Input: Deserializer<'input>,
    {
        #[derive(Deserialize)]
        struct Snapshot {
            version: SourceVersion,
            text: Arc<str>,
            encoding: SourceEncoding,
        }

        let input = Snapshot::deserialize(deserializer)?;

        let snapshot = Self {
            version: input.version,
            text: input.text,
            encoding: input.encoding,
        };

        snapshot.validate().map_err(Input::Error::custom)?;

        Ok(snapshot)
    }
}

/// A zero-based line and UTF-8 byte column, not a protocol character position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinePosition {
    /// Zero-based line number.
    pub line: usize,

    /// UTF-8 byte count from the original line start.
    pub column: usize,
}

/// An owned index over exact original source text, including marker coordinates.
#[derive(Clone, Debug)]
pub struct LineIndex {
    text: Arc<str>,
    starts: Vec<usize>,
}

impl LineIndex {
    fn new(text: Arc<str>) -> Self {
        let mut starts = vec![0];
        let bytes = text.as_bytes();
        let mut offset = 0;

        while offset < bytes.len() {
            match bytes[offset] {
                b'\r' => {
                    offset += 1;

                    if bytes.get(offset) == Some(&b'\n') {
                        offset += 1;
                    }

                    starts.push(offset);
                }

                b'\n' => {
                    offset += 1;
                    starts.push(offset);
                }

                _ => offset += 1,
            }
        }

        Self { text, starts }
    }

    /// Returns the number of lines, including a final empty line after a terminator.
    #[must_use]
    pub fn len(&self) -> usize {
        self.starts.len()
    }

    /// Returns whether the index contains no lines; even empty source has one line.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty()
    }

    /// Maps a validated UTF-8 byte cursor to its original line and byte column.
    ///
    /// # Errors
    /// Returns invalid input for out-of-bounds offsets or split UTF-8 characters.
    pub fn position(&self, offset: usize) -> io::Result<LinePosition> {
        if !self.text.is_char_boundary(offset) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "source offset is not a UTF-8 boundary",
            ));
        }

        let line = self.starts.partition_point(|start| *start <= offset) - 1;

        Ok(LinePosition {
            line,
            column: offset - self.starts[line],
        })
    }

    /// Maps a zero-based line and UTF-8 byte column to an original source offset.
    ///
    /// # Errors
    /// Returns invalid input for absent lines, columns beyond their contents, or split characters.
    pub fn offset(&self, position: LinePosition) -> io::Result<usize> {
        let span = self.span(position.line)?;

        let offset = span
            .start
            .checked_add(position.column)
            .filter(|offset| *offset <= span.end && self.text.is_char_boundary(*offset))
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "line column is outside its UTF-8 contents",
                )
            })?;

        Ok(offset)
    }

    /// Returns a line's original byte span excluding its line terminator.
    ///
    /// # Errors
    /// Returns invalid input when the line does not exist.
    pub fn span(&self, line: usize) -> io::Result<Span> {
        let start = *self.starts.get(line).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "source line is out of bounds")
        })?;

        let mut end = self
            .starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text.len());

        if end > start && self.text.as_bytes()[end - 1] == b'\n' {
            end -= 1;
        }

        if end > start && self.text.as_bytes()[end - 1] == b'\r' {
            end -= 1;
        }

        Ok(Span { start, end })
    }
}

fn reject_encoding(bytes: &[u8]) -> io::Result<()> {
    let unsupported: &[&[u8]] = &[
        b"\xff\xfe",
        b"\xfe\xff",
        b"\x00\x00\xfe\xff",
        b"\x00\x00\xff\xfe",
        b"+/v8",
        b"+/v9",
        b"+/v+",
        b"+/v/",
        b"\xf7\x64\x4c",
        b"\xdd\x73\x66\x73",
        b"\x0e\xfe\xff",
        b"\xfb\xee\x28",
        b"\x84\x31\x95\x33",
    ];

    if unsupported.iter().any(|marker| bytes.starts_with(marker)) {
        return Err(invalid("source encoding is unsupported; expected UTF-8"));
    }

    Ok(())
}
