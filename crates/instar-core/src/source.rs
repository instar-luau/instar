use std::{
    borrow::Cow,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    str,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use line_index::{LineCol, LineIndex, WideEncoding, WideLineCol};
use text_size::{TextRange, TextSize};

static REVISION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
    Utf32,
}

#[derive(Debug)]
pub struct Source {
    path: PathBuf,
    revision: u64,
    bytes: Box<[u8]>,
    lines: LineIndex,
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("source is not a regular file: {0}")]
    NotFile(PathBuf),

    #[error("source requires valid UTF-8: {0}")]
    Encoding(#[from] str::Utf8Error),

    #[error("source exceeds the line-index or revision representation")]
    Capacity,

    #[error("invalid source range or text position")]
    Range,

    #[error("source revision is no longer current")]
    Stale,

    #[error("document is already open")]
    AlreadyOpen,

    #[error("document is not open")]
    NotOpen,

    #[error("document version must increase")]
    Version,
}

impl Source {
    fn new(path: PathBuf, bytes: Vec<u8>) -> Result<Self, SourceError> {
        if u32::try_from(bytes.len()).map_or(true, |len| len == u32::MAX) {
            return Err(SourceError::Capacity);
        }

        let lines = LineIndex::new(&position_text(&bytes));

        let revision = REVISION
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| SourceError::Capacity)?;

        Ok(Self {
            path,
            revision,
            bytes: bytes.into_boxed_slice(),
            lines,
        })
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// # Errors
    /// Returns the original UTF-8 decoding error for byte-only sources.
    pub fn text(&self) -> Result<&str, SourceError> {
        Ok(str::from_utf8(&self.bytes)?)
    }

    /// # Errors
    /// Returns an out-of-bounds range failure.
    pub fn slice(&self, range: TextRange) -> Result<&[u8], SourceError> {
        self.bytes
            .get(usize::from(range.start())..usize::from(range.end()))
            .ok_or(SourceError::Range)
    }

    #[must_use]
    pub fn parse(&self) -> vermis::Tree<'_> {
        vermis::parse(self.bytes().into())
    }

    /// # Errors
    /// Rejects interior character/newline positions and invalid offsets.
    pub fn position(
        &self,
        offset: TextSize,
        encoding: PositionEncoding,
    ) -> Result<LineCol, SourceError> {
        let index = &self.lines;
        let position = index.try_line_col(offset).ok_or(SourceError::Range)?;
        let line = index.line(position.line).ok_or(SourceError::Range)?;
        let content = self.slice(line)?;

        let content = content
            .strip_suffix(b"\n")
            .map_or(content, |text| text.strip_suffix(b"\r").unwrap_or(text));

        if usize::try_from(position.col).map_err(|_| SourceError::Range)? > content.len() {
            return Err(SourceError::Range);
        }

        let wide = match encoding {
            PositionEncoding::Utf8 => return Ok(position),
            PositionEncoding::Utf16 => WideEncoding::Utf16,
            PositionEncoding::Utf32 => WideEncoding::Utf32,
        };

        let position = index.to_wide(wide, position).ok_or(SourceError::Range)?;

        Ok(LineCol {
            line: position.line,
            col: position.col,
        })
    }

    /// # Errors
    /// Rejects invalid lines, columns and surrogate interiors.
    pub fn offset(
        &self,
        position: LineCol,
        encoding: PositionEncoding,
    ) -> Result<TextSize, SourceError> {
        let index = &self.lines;

        let utf8 = match encoding {
            PositionEncoding::Utf8 => position,

            PositionEncoding::Utf16 | PositionEncoding::Utf32 => index
                .to_utf8(
                    match encoding {
                        PositionEncoding::Utf16 => WideEncoding::Utf16,
                        _ => WideEncoding::Utf32,
                    },
                    WideLineCol {
                        line: position.line,
                        col: position.col,
                    },
                )
                .ok_or(SourceError::Range)?,
        };

        let start = index.line(utf8.line).ok_or(SourceError::Range)?.start();

        let offset = u32::from(start)
            .checked_add(utf8.col)
            .ok_or(SourceError::Range)?;

        let offset = TextSize::from(offset);

        if self.position(offset, encoding)? != position {
            return Err(SourceError::Range);
        }

        Ok(offset)
    }
}

fn position_text(bytes: &[u8]) -> Cow<'_, str> {
    let error = match str::from_utf8(bytes) {
        Ok(text) => return Cow::Borrowed(text),
        Err(error) => error,
    };

    let mut view = bytes.to_vec();
    let mut at = error.valid_up_to();

    loop {
        match str::from_utf8(&view[at..]) {
            Ok(_) => break,

            Err(error) => {
                at += error.valid_up_to();
                let end = at + error.error_len().unwrap_or(view.len() - at);
                view[at..end].fill(0x1a);
                at = end;
            }
        }
    }

    Cow::Owned(String::from_utf8(view).expect("malformed bytes replaced"))
}

#[derive(Debug)]
struct Entry {
    source: Arc<Source>,
    version: Option<i32>,
}

#[derive(Debug, Default)]
pub struct SourceStore {
    entries: BTreeMap<PathBuf, Entry>,
}

pub(crate) fn absolute(path: &Path) -> Result<PathBuf, SourceError> {
    let path = std::path::absolute(path).map_err(|source| SourceError::Io {
        path: path.to_owned(),
        source,
    })?;

    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}

            std::path::Component::ParentDir => {
                normalized.pop();
            }

            _ => normalized.push(component),
        }
    }

    Ok(normalized)
}

impl SourceStore {
    /// # Errors
    /// Returns path normalization failures.
    pub fn is_open(&self, path: &Path) -> Result<bool, SourceError> {
        Ok(self
            .entries
            .get(&absolute(path)?)
            .is_some_and(|entry| entry.version.is_some()))
    }

    pub(crate) fn has_open_descendants(&self, path: &Path) -> Result<bool, SourceError> {
        let path = absolute(path)?;

        Ok(self.entries.iter().any(|(candidate, entry)| {
            entry.version.is_some() && candidate != &path && candidate.starts_with(&path)
        }))
    }

    /// # Errors
    /// Returns path, duplicate-document or source capacity failures.
    pub fn open_bytes(
        &mut self,
        path: &Path,
        version: i32,
        bytes: Vec<u8>,
    ) -> Result<Arc<Source>, SourceError> {
        let path = absolute(path)?;

        if self
            .entries
            .get(&path)
            .is_some_and(|entry| entry.version.is_some())
        {
            return Err(SourceError::AlreadyOpen);
        }

        let source = Arc::new(Source::new(path.clone(), bytes)?);

        self.entries.insert(
            path,
            Entry {
                source: Arc::clone(&source),
                version: Some(version),
            },
        );

        Ok(source)
    }

    /// # Errors
    /// Returns filesystem or representational failures without replacing the current source.
    pub fn read(&mut self, path: &Path) -> Result<Arc<Source>, SourceError> {
        let path = absolute(path)?;

        if let Some(entry) = self.entries.get(&path)
            && entry.version.is_some()
        {
            return Ok(Arc::clone(&entry.source));
        }

        let metadata = fs::metadata(&path).map_err(|source| SourceError::Io {
            path: path.clone(),
            source,
        })?;

        if !metadata.is_file() {
            return Err(SourceError::NotFile(path));
        }

        let bytes = fs::read(&path).map_err(|source| SourceError::Io {
            path: path.clone(),
            source,
        })?;

        if let Some(entry) = self.entries.get(&path)
            && entry.source.bytes() == bytes
        {
            return Ok(Arc::clone(&entry.source));
        }

        let source = Arc::new(Source::new(path.clone(), bytes)?);

        self.entries.insert(
            path,
            Entry {
                source: Arc::clone(&source),
                version: None,
            },
        );

        Ok(source)
    }

    /// # Errors
    /// Returns an already-open, path or representational failure.
    pub fn open(
        &mut self,
        path: &Path,
        version: i32,
        text: &str,
    ) -> Result<Arc<Source>, SourceError> {
        self.open_bytes(path, version, text.as_bytes().to_vec())
    }

    /// # Errors
    /// Returns stale, unopened, non-increasing-version or representational failures.
    pub fn update(
        &mut self,
        expected: &Source,
        version: i32,
        text: &str,
    ) -> Result<Arc<Source>, SourceError> {
        self.validate(expected)?;

        let entry = self
            .entries
            .get_mut(expected.path())
            .ok_or(SourceError::Stale)?;

        let previous = entry.version.ok_or(SourceError::NotOpen)?;

        if version <= previous {
            return Err(SourceError::Version);
        }

        let source = Arc::new(Source::new(
            expected.path.clone(),
            text.as_bytes().to_vec(),
        )?);

        entry.source = Arc::clone(&source);
        entry.version = Some(version);

        Ok(source)
    }

    /// # Errors
    /// Returns stale or unopened document failures.
    pub fn close(&mut self, expected: &Source) -> Result<(), SourceError> {
        self.validate(expected)?;

        if self
            .entries
            .get(expected.path())
            .is_none_or(|entry| entry.version.is_none())
        {
            return Err(SourceError::NotOpen);
        }

        self.entries.remove(expected.path());

        Ok(())
    }

    /// # Errors
    /// Returns stale for replaced, closed or foreign-store snapshots.
    pub fn validate(&self, expected: &Source) -> Result<(), SourceError> {
        if self
            .entries
            .get(expected.path())
            .is_some_and(|entry| entry.source.revision == expected.revision)
        {
            Ok(())
        } else {
            Err(SourceError::Stale)
        }
    }
}
