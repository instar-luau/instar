use std::{
    iter,
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use instar_core::analysis::{self, Analysis};

use tower_lsp_server::{
    gen_lsp_types::{
        Position, PositionEncodingKind, Range as ProtocolRange, TextDocumentContentChangeEvent, Uri,
    },
    jsonrpc::{Error, Result},
};

#[derive(Clone)]
pub(super) struct Document {
    pub(super) uri: Uri,
    pub(super) text: Arc<str>,
    pub(super) version: Option<i32>,
    pub(super) revision: u64,
    pub(super) analysis: Arc<Analysis>,
    lines: Arc<[usize]>,
}

impl Document {
    pub(super) fn new(uri: Uri, text: Arc<str>, version: Option<i32>, revision: u64) -> Self {
        let lines = lines(&text);

        let analysis = Arc::new(analysis::analyze(&text));

        Self {
            uri,
            text,
            version,
            revision,
            analysis,
            lines,
        }
    }

    pub(super) fn offset(&self, position: Position, encoding: Encoding) -> Result<usize> {
        offset(&self.text, &self.lines, position, encoding)
    }

    pub(super) fn position(&self, offset: usize, encoding: Encoding) -> Result<Position> {
        if !self.text.is_char_boundary(offset) {
            return Err(Error::invalid_params(
                "Source offset is outside the document or splits a character.",
            ));
        }

        let line = self
            .lines
            .partition_point(|start| *start <= offset)
            .saturating_sub(1);

        let text = self.text[self.lines[line]..offset].trim_end_matches(['\r', '\n']);

        let character: usize = text
            .chars()
            .map(|character| encoding.width(character))
            .sum();

        Ok(Position {
            line: u32::try_from(line).map_err(|error| Error::invalid_params(error.to_string()))?,
            character: u32::try_from(character)
                .map_err(|error| Error::invalid_params(error.to_string()))?,
        })
    }

    pub(super) fn range(&self, range: Range<usize>, encoding: Encoding) -> Result<ProtocolRange> {
        if range.start > range.end {
            return Err(Error::invalid_params("Source range is reversed."));
        }

        Ok(ProtocolRange {
            start: self.position(range.start, encoding)?,
            end: self.position(range.end, encoding)?,
        })
    }

    pub(super) fn bytes(&self, range: ProtocolRange, encoding: Encoding) -> Result<Range<usize>> {
        bytes(&self.text, &self.lines, range, encoding)
    }

    pub(super) fn native(&self, position: Position, encoding: Encoding) -> Result<(u32, u32)> {
        let offset = self.offset(position, encoding)?;

        let start = self.lines[usize::try_from(position.line)
            .map_err(|error| Error::invalid_params(error.to_string()))?];

        Ok((
            position.line,
            u32::try_from(offset - start)
                .map_err(|error| Error::invalid_params(error.to_string()))?,
        ))
    }

    pub(super) fn native_range(
        &self,
        range: instar_bridge::Range,
        encoding: Encoding,
    ) -> Result<ProtocolRange> {
        self.range(self.native_bytes(range)?, encoding)
    }

    pub(super) fn native_bytes(&self, range: instar_bridge::Range) -> Result<Range<usize>> {
        self.bytes(
            ProtocolRange {
                start: Position {
                    line: range.start.line,
                    character: range.start.column,
                },
                end: Position {
                    line: range.end.line,
                    character: range.end.column,
                },
            },
            Encoding::Bytes,
        )
    }

    pub(super) fn changed(
        &self,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        encoding: Encoding,
        revision: u64,
    ) -> Result<Self> {
        if self.version.is_some_and(|previous| version <= previous) {
            return Err(Error::content_modified());
        }

        let mut text = self.text.to_string();

        for change in changes {
            match change {
                TextDocumentContentChangeEvent::TextDocumentContentChangePartial(change) => {
                    let lines = lines(&text);
                    let range = bytes(&text, &lines, change.range, encoding)?;
                    text.replace_range(range, &change.text);
                }

                TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(change) => {
                    text = change.text;
                }
            }
        }

        Ok(Self::new(
            self.uri.clone(),
            Arc::from(text),
            Some(version),
            revision,
        ))
    }
}

#[derive(Copy, Clone, Default, Eq, PartialEq)]
pub(super) enum Encoding {
    Bytes,

    #[default]
    Units,

    Scalars,
}

impl Encoding {
    pub(super) fn protocol(self) -> PositionEncodingKind {
        match self {
            Self::Bytes => PositionEncodingKind::UTF8,
            Self::Units => PositionEncodingKind::UTF16,
            Self::Scalars => PositionEncodingKind::UTF32,
        }
    }

    fn width(self, character: char) -> usize {
        match self {
            Self::Bytes => character.len_utf8(),
            Self::Units => character.len_utf16(),
            Self::Scalars => 1,
        }
    }
}

fn lines(text: &str) -> Arc<[usize]> {
    iter::once(0)
        .chain(text.match_indices('\n').map(|(offset, _)| offset + 1))
        .collect()
}

fn offset(source: &str, lines: &[usize], position: Position, encoding: Encoding) -> Result<usize> {
    let line =
        usize::try_from(position.line).map_err(|error| Error::invalid_params(error.to_string()))?;

    let start = *lines
        .get(line)
        .ok_or_else(|| Error::invalid_params("Source line is outside the document."))?;

    let end = lines.get(line + 1).copied().unwrap_or(source.len());
    let text = source[start..end].trim_end_matches(['\r', '\n']);

    let expected = usize::try_from(position.character)
        .map_err(|error| Error::invalid_params(error.to_string()))?;

    let mut units = 0;

    for (offset, character) in text.char_indices() {
        if units == expected {
            return Ok(start + offset);
        }

        units += encoding.width(character);

        if units > expected {
            return Err(Error::invalid_params("Source position splits a character."));
        }
    }

    if units == expected {
        Ok(start + text.len())
    } else {
        Err(Error::invalid_params("Source column is outside the line."))
    }
}

fn bytes(
    text: &str,
    lines: &[usize],
    range: ProtocolRange,
    encoding: Encoding,
) -> Result<Range<usize>> {
    let start = offset(text, lines, range.start, encoding)?;
    let end = offset(text, lines, range.end, encoding)?;

    if start > end {
        return Err(Error::invalid_params("Source range is reversed."));
    }

    Ok(start..end)
}

pub(super) fn path(uri: &Uri) -> Result<PathBuf> {
    uri.to_file_path()
        .map_err(|()| Error::invalid_params("Document URI must identify a local file."))
}

pub(super) fn uri(path: &Path) -> Result<Uri> {
    Uri::from_file_path(path)
        .map_err(|()| Error::invalid_params("Source path cannot be represented as a file URI."))
}
