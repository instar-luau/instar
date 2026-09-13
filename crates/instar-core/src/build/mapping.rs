use crate::graft::Mapping;
use serde::Serialize;
use std::{
    collections::BTreeMap,
    io,
    ops::Range,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize)]
pub(super) struct Segment {
    #[serde(flatten)]
    pub range: Mapping,

    pub source: PathBuf,

    #[serde(skip)]
    pub linear: bool,
}

#[derive(Clone, Default)]
pub(super) struct Text {
    pub text: String,
    pub segments: Vec<Segment>,
}

#[derive(Clone)]
pub(super) struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Text {
    pub(super) fn original(path: &Path, text: String) -> Self {
        let length = text.len();

        Self {
            text,
            segments: if length == 0 {
                Vec::new()
            } else {
                vec![Segment {
                    source: path.to_owned(),
                    linear: true,
                    range: Mapping {
                        start: 0,
                        end: length,
                        original_start: 0,
                        original_end: length,
                    },
                }]
            },
        }
    }

    pub(super) fn append(&mut self, other: &Self) {
        let offset = self.text.len();
        self.text.push_str(&other.text);

        self.segments
            .extend(other.segments.iter().cloned().map(|mut segment| {
                segment.range.start += offset;
                segment.range.end += offset;

                segment
            }));
    }

    pub(super) fn generated(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub(super) fn origin(&self, offset: usize) -> Option<(&Path, usize)> {
        self.segments
            .get(
                self.segments
                    .partition_point(|segment| segment.range.end <= offset),
            )
            .filter(|segment| segment.range.start <= offset)
            .map(|segment| (segment.source.as_path(), original(segment, offset)))
    }

    fn section(&self, range: Range<usize>) -> Self {
        let mut output = Self {
            text: self.text[range.clone()].to_owned(),
            segments: Vec::new(),
        };

        for segment in self
            .segments
            .iter()
            .skip(
                self.segments
                    .partition_point(|segment| segment.range.end <= range.start),
            )
            .take_while(|segment| segment.range.start < range.end)
        {
            let start = range.start.max(segment.range.start);
            let end = range.end.min(segment.range.end);

            if start < end {
                output.segments.push(Segment {
                    source: segment.source.clone(),
                    linear: segment.linear,
                    range: Mapping {
                        start: start - range.start,
                        end: end - range.start,
                        original_start: original(segment, start),
                        original_end: original(segment, end),
                    },
                });
            }
        }

        output
    }

    pub(super) fn edit(&self, mut edits: Vec<Edit>) -> io::Result<Self> {
        edits.sort_by_key(|edit| (edit.range.start, edit.range.end));
        let mut output = Self::default();
        let mut previous = 0;

        for edit in edits {
            if edit.range.start < previous || self.text.get(edit.range.clone()).is_none() {
                return Err(io::Error::other("overlapping or invalid build edits"));
            }

            output.append(&self.section(previous..edit.range.start));
            let start = output.text.len();
            output.generated(&edit.text);

            if !edit.text.is_empty()
                && let Some((source, original_start)) = self.origin(edit.range.start)
            {
                output.segments.push(Segment {
                    source: source.to_owned(),
                    linear: false,
                    range: Mapping {
                        start,
                        end: output.text.len(),
                        original_start,
                        original_end: original_start,
                    },
                });
            }

            previous = edit.range.end;
        }

        output.append(&self.section(previous..self.text.len()));

        Ok(output)
    }

    pub(super) fn map(
        &self,
        root: &Path,
        output: &Path,
        originals: &BTreeMap<PathBuf, String>,
    ) -> io::Result<Vec<u8>> {
        let sources = self
            .segments
            .iter()
            .map(|segment| segment.source.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();

        let generated_index = line_index::LineIndex::new(&self.text);

        let original_indexes = originals
            .iter()
            .map(|(path, text)| (path.clone(), line_index::LineIndex::new(text)))
            .collect::<BTreeMap<_, _>>();

        let mut points = vermis::tokenize(self.text.as_bytes().into())
            .iter()
            .map(|token| token.span.start)
            .collect::<Vec<_>>();

        points.extend(
            self.text
                .match_indices('\n')
                .map(|(position, _)| position + 1),
        );

        points.sort_unstable();
        points.dedup();
        let mut mappings = String::new();
        let mut generated_line = 0;
        let mut generated_column = 0;
        let mut previous_source = 0;
        let mut previous_line = 0;
        let mut previous_column = 0;
        let mut separated = false;

        for offset in points {
            let (line, column) = position(&generated_index, offset)?;

            while generated_line < line {
                mappings.push(';');
                generated_line += 1;
                generated_column = 0;
                separated = false;
            }

            if separated {
                mappings.push(',');
            }

            variable(column - generated_column, &mut mappings);
            generated_column = column;

            if let Some((source, original_offset)) = self.origin(offset) {
                let source_index = i64::try_from(
                    sources
                        .binary_search(&source.to_owned())
                        .map_err(|_| io::Error::other("mapping source index missing"))?,
                )
                .map_err(io::Error::other)?;

                let index = original_indexes
                    .get(source)
                    .ok_or_else(|| io::Error::other("mapping source unavailable"))?;

                let (line, column) = position(index, original_offset)?;
                variable(source_index - previous_source, &mut mappings);
                variable(line - previous_line, &mut mappings);
                variable(column - previous_column, &mut mappings);
                previous_source = source_index;
                previous_line = line;
                previous_column = column;
            }

            separated = true;
        }

        let mut ranges = self.segments.clone();

        for segment in &mut ranges {
            segment.source = segment
                .source
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_owned();
        }

        let mut document = serde_json::json!({"version":3,"file":output.file_name().and_then(|name| name.to_str()),"sources":sources.iter().map(|path| path.strip_prefix(root).unwrap_or(path).to_string_lossy().replace('\\', "/")).collect::<Vec<_>>(),"sourcesContent":sources.iter().map(|path| &originals[path]).collect::<Vec<_>>(),"names":[],"mappings":mappings,"x_instar_ranges":ranges});

        let source_root = super::paths::between(
            output
                .parent()
                .ok_or_else(|| io::Error::other("mapped output has no parent"))?,
            root,
        )?;

        if source_root != "./" {
            document["sourceRoot"] = serde_json::Value::String(source_root);
        }

        serde_json::to_vec(&document).map_err(io::Error::other)
    }
}

fn original(segment: &Segment, offset: usize) -> usize {
    let range = &segment.range;

    if segment.linear {
        range.original_start + offset - range.start
    } else {
        range.original_start
    }
}

fn position(index: &line_index::LineIndex, offset: usize) -> io::Result<(i64, i64)> {
    let offset = u32::try_from(offset).map_err(io::Error::other)?;

    let position = index
        .try_line_col(offset.into())
        .and_then(|position| index.to_wide(line_index::WideEncoding::Utf16, position))
        .ok_or_else(|| io::Error::other("invalid source mapping offset"))?;

    Ok((i64::from(position.line), i64::from(position.col)))
}

fn variable(value: i64, output: &mut String) {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut value = (value.unsigned_abs() << 1) | u64::from(value < 0);

    loop {
        let mut digit = (value & 31) as usize;
        value >>= 5;

        if value != 0 {
            digit |= 32;
        }

        output.push(char::from(ALPHABET[digit]));

        if value == 0 {
            break;
        }
    }
}
