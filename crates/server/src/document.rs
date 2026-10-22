use std::{
    ops::Range,
    path::{Path, PathBuf},
    result,
    sync::{Arc, Mutex, OnceLock},
};

use instar_core::{
    analysis::{self, Analysis},
    lint,
};

use line_index::{LineCol, LineIndex, TextSize, WideEncoding, WideLineCol};

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
    analysis: Arc<OnceLock<Analysis>>,
    findings: Arc<Mutex<Option<Lint>>>,
    lines: Arc<LineIndex>,
}

struct Lint {
    settings: Arc<lint::Prepared>,
    symbols: Arc<lint::Symbols>,
    findings: result::Result<Arc<[lint::Finding]>, lint::Failure>,
}

impl Document {
    pub(super) fn new(
        uri: Uri,
        text: Arc<str>,
        version: Option<i32>,
        revision: u64,
    ) -> Result<Self> {
        let lines = Arc::new(lines(&text)?);

        Ok(Self {
            uri,
            text,
            version,
            revision,
            analysis: Arc::new(OnceLock::new()),
            findings: Arc::new(Mutex::new(None)),
            lines,
        })
    }

    pub(super) fn analysis(&self) -> &Analysis {
        self.analysis
            .get_or_init(|| analysis::analyze(Arc::clone(&self.text)))
    }

    pub(super) fn findings(
        &self,
        settings: Arc<lint::Prepared>,
        symbols: Arc<lint::Symbols>,
    ) -> result::Result<Arc<[lint::Finding]>, lint::Failure> {
        let mut cached = self
            .findings
            .lock()
            .expect("Internal error: lint cache mutex is poisoned.");

        if let Some(cached) = &*cached
            && Arc::ptr_eq(&cached.settings, &settings)
            && Arc::ptr_eq(&cached.symbols, &symbols)
        {
            return cached.findings.clone();
        }

        let findings = lint::lint(&self.text, &settings, &symbols).map(Arc::from);

        *cached = Some(Lint {
            settings,
            symbols,
            findings: findings.clone(),
        });

        findings
    }

    pub(super) fn offset(&self, position: Position, encoding: Encoding) -> Result<usize> {
        offset(&self.text, &self.lines, position, encoding)
    }

    pub(super) fn position(&self, offset: usize, encoding: Encoding) -> Result<Position> {
        let position = TextSize::try_from(offset)
            .ok()
            .and_then(|offset| self.lines.try_line_col(offset))
            .ok_or_else(|| {
                Error::invalid_params(
                    "Source offset is outside the document or splits a character.",
                )
            })?;

        let start = self
            .lines
            .offset(LineCol {
                line: position.line,
                col: 0,
            })
            .expect("Internal error: indexed line is missing.");

        let text = self.text[usize::from(start)..offset].trim_end_matches(['\r', '\n']);

        let position = LineCol {
            line: position.line,
            col: u32::try_from(text.len())
                .map_err(|error| Error::invalid_params(error.to_string()))?,
        };

        let character = match encoding.wide() {
            Some(encoding) => {
                self.lines
                    .to_wide(encoding, position)
                    .ok_or_else(|| Error::invalid_params("Source column is outside the line."))?
                    .col
            }

            None => position.col,
        };

        Ok(Position {
            line: position.line,
            character,
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

        let position = self.lines.line_col(
            TextSize::try_from(offset).map_err(|error| Error::invalid_params(error.to_string()))?,
        );

        Ok((position.line, position.col))
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
                    let lines = lines(&text)?;
                    let range = bytes(&text, &lines, change.range, encoding)?;
                    text.replace_range(range, &change.text);
                }

                TextDocumentContentChangeEvent::TextDocumentContentChangeWholeDocument(change) => {
                    text = change.text;
                }
            }
        }

        Self::new(self.uri.clone(), Arc::from(text), Some(version), revision)
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

    fn wide(self) -> Option<WideEncoding> {
        match self {
            Self::Bytes => None,
            Self::Units => Some(WideEncoding::Utf16),
            Self::Scalars => Some(WideEncoding::Utf32),
        }
    }
}

fn lines(text: &str) -> Result<LineIndex> {
    u32::try_from(text.len())
        .ok()
        .filter(|length| *length < u32::MAX)
        .ok_or_else(|| Error::invalid_params("Document exceeds the 32-bit source size limit."))?;

    Ok(LineIndex::new(text))
}

fn offset(
    source: &str,
    lines: &LineIndex,
    position: Position,
    encoding: Encoding,
) -> Result<usize> {
    let range = lines
        .line(position.line)
        .ok_or_else(|| Error::invalid_params("Source line is outside the document."))?;

    let start = usize::from(range.start());
    let text = source[start..usize::from(range.end())].trim_end_matches(['\r', '\n']);

    let position = match encoding.wide() {
        Some(encoding) => lines
            .to_utf8(
                encoding,
                WideLineCol {
                    line: position.line,
                    col: position.character,
                },
            )
            .ok_or_else(|| Error::invalid_params("Source column is outside the line."))?,

        None => LineCol {
            line: position.line,
            col: position.character,
        },
    };

    let column =
        usize::try_from(position.col).map_err(|error| Error::invalid_params(error.to_string()))?;

    if column > text.len() {
        return Err(Error::invalid_params("Source column is outside the line."));
    }

    if !text.is_char_boundary(column) {
        return Err(Error::invalid_params("Source position splits a character."));
    }

    Ok(start + column)
}

fn bytes(
    text: &str,
    lines: &LineIndex,
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
