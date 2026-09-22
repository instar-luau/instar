use std::{
    io,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use serde_json::{Value, json};
use tower_lsp_server::ls_types::{Position, Range, SymbolKind, Uri};
use vermis::{Kind, Parts};

use crate::bindings::Index;

pub(crate) struct Document {
    pub(crate) uri: Uri,
    pub(crate) path: PathBuf,
    pub(crate) version: i32,
    pub(crate) text: String,
    lines: Vec<usize>,
    syntax: OnceLock<Syntax>,
}

struct Syntax {
    spans: Vec<(usize, usize)>,
    symbols: Vec<Value>,
    folds: Vec<Value>,
    bindings: Index,
}

pub(crate) fn path(uri: &Uri) -> io::Result<PathBuf> {
    url::Url::parse(uri.as_str())
        .map_err(io::Error::other)?
        .to_file_path()
        .map_err(|()| io::Error::other("only file URIs are supported"))
}

pub(crate) fn uri(path: &Path) -> io::Result<Uri> {
    url::Url::from_file_path(path)
        .map_err(|()| io::Error::other("invalid file path"))?
        .as_str()
        .parse()
        .map_err(io::Error::other)
}

impl Document {
    pub(crate) fn new(uri: Uri, version: i32, text: String) -> io::Result<Self> {
        let path = path(&uri)?;

        let lines = std::iter::once(0)
            .chain(
                text.bytes()
                    .enumerate()
                    .filter_map(|(index, byte)| (byte == b'\n').then_some(index + 1)),
            )
            .collect();

        Ok(Self {
            uri,
            path,
            version,
            text,
            lines,
            syntax: OnceLock::new(),
        })
    }

    pub(crate) fn offset(&self, position: Position) -> io::Result<usize> {
        let line = usize::try_from(position.line).map_err(io::Error::other)?;

        let Some(&start) = self.lines.get(line) else {
            return Ok(self.text.len());
        };

        let end = self.lines.get(line + 1).copied().unwrap_or(self.text.len());
        let text = self.text[start..end].trim_end_matches(['\r', '\n']);
        let mut units = 0;

        for (offset, ch) in text.char_indices() {
            if units == position.character {
                return Ok(start + offset);
            }

            units += u32::try_from(ch.len_utf16()).map_err(io::Error::other)?;

            if units > position.character {
                return Err(io::Error::other("position splits a UTF-16 surrogate pair"));
            }
        }

        Ok(start + text.len())
    }

    pub(crate) fn position(&self, mut offset: usize) -> Position {
        offset = offset.min(self.text.len());

        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }

        let line = self
            .lines
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);

        Position::new(
            u32::try_from(line).unwrap_or(u32::MAX),
            u32::try_from(self.text[self.lines[line]..offset].encode_utf16().count())
                .unwrap_or(u32::MAX),
        )
    }

    pub(crate) fn byte_position(&self, position: Position) -> io::Result<(u32, u32)> {
        let offset = self.offset(position)?;

        let line = self
            .lines
            .partition_point(|&start| start <= offset)
            .saturating_sub(1);

        Ok((
            u32::try_from(line).map_err(io::Error::other)?,
            u32::try_from(offset - self.lines[line]).map_err(io::Error::other)?,
        ))
    }

    pub(crate) fn range(&self, start: usize, end: usize) -> Range {
        Range::new(self.position(start), self.position(end))
    }

    pub(crate) fn native_range(&self, range: [u32; 4]) -> Range {
        let offset = |line, column| {
            self.lines
                .get(usize::try_from(line).unwrap_or(usize::MAX))
                .copied()
                .unwrap_or(self.text.len())
                .saturating_add(usize::try_from(column).unwrap_or(usize::MAX))
        };

        self.range(offset(range[0], range[1]), offset(range[2], range[3]))
    }

    fn syntax(&self) -> &Syntax {
        self.syntax.get_or_init(|| {
            let tree = vermis::parse(self.text.as_bytes());
            let mut syntax = Syntax { spans: Vec::new(), symbols: Vec::new(), folds: Vec::new(), bindings: Index::new(&tree) };

            for (index, node) in tree.nodes.iter().enumerate() {
                let span = node.span;

                if span.end > self.text.len() || span.start > span.end { continue; }

                syntax.spans.push((span.start, span.end));
                let range = self.range(span.start, span.end);

                if range.end.line > range.start.line && matches!(node.kind,
                    Kind::Function | Kind::LocalFunction | Kind::If | Kind::While | Kind::Repeat |
                    Kind::NumericFor | Kind::GenericFor | Kind::Do | Kind::Table | Kind::TypeTable |
                    Kind::Class | Kind::String)
                {
                    syntax.folds.push(json!({"startLine": range.start.line, "startCharacter": range.start.character,
                        "endLine": range.end.line, "endCharacter": range.end.character, "kind": "region"}));
                }

                let mut symbol = |name: vermis::View<'_, '_>, kind: SymbolKind| {
                    let selection = name.span();

                    syntax.symbols.push(json!({"name": String::from_utf8_lossy(name.text()), "kind": kind,
                        "range": range, "selectionRange": self.range(selection.start, selection.end)}));
                };

                match tree.view(index).and_then(vermis::View::parts) {
                    Some(Parts::Function { name: Some(name), .. }) => symbol(name, SymbolKind::FUNCTION),

                    Some(Parts::Local { bindings, values }) => {
                        let values = values.collect::<Vec<_>>();

                        for (i, binding) in bindings.enumerate() {
                            if let Some(Parts::Binding { name, .. }) = binding.parts() {
                                let kind = if values.get(i).is_some_and(|value| value.kind() == Kind::Function) { SymbolKind::FUNCTION } else { SymbolKind::VARIABLE };

                                symbol(name, kind);
                            }
                        }
                    }

                    Some(Parts::TypeAlias { name, .. } | Parts::Class { name, .. }) => symbol(name, SymbolKind::CLASS),
                    _ => {}
                }
            }

            syntax.spans.sort_unstable();
            syntax.spans.dedup();

            syntax
        })
    }

    pub(crate) fn bindings(&self) -> &Index {
        &self.syntax().bindings
    }

    pub(crate) fn tokens(&self) -> Value {
        let mut data = Vec::<u32>::new();
        let mut previous = (0, 0);

        for (span, kind, modifiers) in self.bindings().tokens() {
            let range = self.range(span.start, span.end);

            for line in range.start.line..=range.end.line {
                let start = if line == range.start.line {
                    range.start.character
                } else {
                    0
                };

                let end = if line == range.end.line {
                    range.end.character
                } else {
                    let index = usize::try_from(line).unwrap_or(usize::MAX);
                    let start = self.lines[index];

                    let end = self
                        .lines
                        .get(index + 1)
                        .copied()
                        .unwrap_or(self.text.len());

                    u32::try_from(
                        self.text[start..end]
                            .trim_end_matches(['\r', '\n'])
                            .encode_utf16()
                            .count(),
                    )
                    .unwrap_or(u32::MAX)
                };

                if end > start {
                    data.extend([
                        line - previous.0,
                        if line == previous.0 {
                            start - previous.1
                        } else {
                            start
                        },
                        end - start,
                        kind as u32,
                        modifiers,
                    ]);

                    previous = (line, start);
                }
            }
        }

        json!({"data": data})
    }

    pub(crate) fn symbols(&self) -> Value {
        json!(self.syntax().symbols)
    }

    pub(crate) fn folds(&self) -> Value {
        json!(self.syntax().folds)
    }

    pub(crate) fn selections(&self, positions: &[Position]) -> io::Result<Value> {
        let mut selections = Vec::new();

        for position in positions {
            let offset = self.offset(*position)?;

            let mut spans = self
                .syntax()
                .spans
                .iter()
                .copied()
                .filter(|&(start, end)| start <= offset && offset <= end)
                .collect::<Vec<_>>();

            spans.sort_by_key(|&(start, end)| std::cmp::Reverse(end - start));
            let mut parent = Value::Null;
            let mut enclosing = (0, self.text.len());

            for (start, end) in spans {
                if start < enclosing.0 || end > enclosing.1 {
                    continue;
                }

                let mut value = json!({"range": self.range(start, end)});

                if !parent.is_null() {
                    value["parent"] = parent;
                }

                parent = value;
                enclosing = (start, end);
            }

            if parent.is_null() {
                parent = json!({"range": self.range(offset, offset)});
            }

            selections.push(parent);
        }

        Ok(json!(selections))
    }
}
