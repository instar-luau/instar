use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use serde_json::{Value, json};
use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, Location, Position, Range,
    SymbolKind, Uri,
};
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
    features: crate::features::Syntax,
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

pub(crate) fn diagnostics(
    findings: impl IntoIterator<Item = instar_core::analysis::Diagnostic>,
    mut document: impl FnMut(&Path) -> io::Result<Arc<Document>>,
) -> io::Result<BTreeMap<PathBuf, Vec<Diagnostic>>> {
    use instar_core::analysis::Severity;

    let mut reports: BTreeMap<PathBuf, Vec<Diagnostic>> = BTreeMap::new();

    for finding in findings {
        let path = finding.location.module.source;
        let source = document(&path)?;
        let related_information = match finding.related {
            Some((location, message)) => {
                let target = document(&location.module.source)?;
                Some(vec![DiagnosticRelatedInformation {
                    location: Location {
                        uri: target.uri.clone(),
                        range: target.native_range(location.range),
                    },
                    message,
                }])
            }
            None => None,
        };

        reports.entry(path).or_default().push(Diagnostic {
            range: source.native_range(finding.location.range),
            severity: Some(match finding.severity {
                Severity::Error => DiagnosticSeverity::ERROR,
                Severity::Warning => DiagnosticSeverity::WARNING,
                Severity::Information => DiagnosticSeverity::INFORMATION,
            }),
            source: Some("instar".into()),
            message: finding.message,
            related_information,
            ..Diagnostic::default()
        });
    }

    for items in reports.values_mut() {
        items.sort_unstable_by(|left, right| {
            (
                left.range.start,
                left.range.end,
                left.severity,
                &left.message,
            )
                .cmp(&(
                    right.range.start,
                    right.range.end,
                    right.severity,
                    &right.message,
                ))
                .then_with(|| {
                    left.related_information
                        .iter()
                        .flatten()
                        .map(|item| {
                            (
                                item.location.uri.as_str(),
                                item.location.range.start,
                                item.location.range.end,
                                item.message.as_str(),
                            )
                        })
                        .cmp(right.related_information.iter().flatten().map(|item| {
                            (
                                item.location.uri.as_str(),
                                item.location.range.start,
                                item.location.range.end,
                                item.message.as_str(),
                            )
                        }))
                })
        });
        items.dedup();
    }

    Ok(reports)
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
            let mut syntax = Syntax { spans: Vec::new(), symbols: Vec::new(), folds: Vec::new(), bindings: Index::new(&tree), features: crate::features::Syntax::new(&tree) };

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

    pub(crate) fn features(&self) -> &crate::features::Syntax {
        &self.syntax().features
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

#[cfg(test)]
mod tests {
    use super::*;
    use instar_core::{
        analysis::{Diagnostic as Finding, Location as SourceLocation, Severity},
        resolve::Module,
    };

    #[test]
    fn diagnostic_batches_convert_sort_deduplicate_and_propagate_resolution_errors() {
        let directory = std::env::temp_dir();
        let source = uri(&directory.join("diagnostics.luau")).unwrap();
        let source = Arc::new(Document::new(source, 1, "𐐀a\n".into()).unwrap());
        let related = uri(&directory.join("related.luau")).unwrap();
        let related = Arc::new(Document::new(related, 1, "prefix\n𐐀b\n".into()).unwrap());
        let location = |document: &Document, range| SourceLocation {
            module: Module {
                path: document.path.clone(),
                source: document.path.clone(),
                instance: None,
            },
            range,
        };
        let finding = |severity, message: &str, related_message: &str| Finding {
            location: location(&source, [0, 4, 0, 5]),
            severity,
            message: message.into(),
            related: Some((location(&related, [1, 4, 1, 5]), related_message.into())),
        };
        let documents = BTreeMap::from([
            (source.path.clone(), Arc::clone(&source)),
            (related.path.clone(), Arc::clone(&related)),
        ]);
        let resolve = |path: &Path| {
            documents
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "diagnostic source missing"))
        };
        let findings = vec![
            finding(Severity::Information, "information", "context"),
            finding(Severity::Error, "error", "z"),
            finding(Severity::Warning, "warning", "context"),
            finding(Severity::Error, "error", "a"),
            finding(Severity::Error, "error", "z"),
        ];
        let reports = diagnostics(findings.clone(), resolve).unwrap();
        let items = &reports[&source.path];

        assert_eq!(
            items
                .iter()
                .map(|item| (
                    item.severity,
                    item.message.as_str(),
                    item.related_information.as_ref().unwrap()[0]
                        .message
                        .as_str(),
                ))
                .collect::<Vec<_>>(),
            vec![
                (Some(DiagnosticSeverity::ERROR), "error", "a"),
                (Some(DiagnosticSeverity::ERROR), "error", "z"),
                (Some(DiagnosticSeverity::WARNING), "warning", "context"),
                (
                    Some(DiagnosticSeverity::INFORMATION),
                    "information",
                    "context"
                ),
            ]
        );
        for item in items {
            assert_eq!(
                item.range,
                Range::new(Position::new(0, 2), Position::new(0, 3))
            );
            assert_eq!(item.source.as_deref(), Some("instar"));
            let information = &item.related_information.as_ref().unwrap()[0];
            assert_eq!(information.location.uri, related.uri);
            assert_eq!(
                information.location.range,
                Range::new(Position::new(1, 2), Position::new(1, 3))
            );
        }
        assert_eq!(
            diagnostics(findings.clone().into_iter().rev(), resolve).unwrap(),
            reports
        );

        let mut missing = finding(Severity::Error, "missing", "context");
        missing.related.as_mut().unwrap().0.module.source = directory.join("missing.luau");
        assert_eq!(
            diagnostics([findings[0].clone(), missing], resolve)
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
        let mut missing = findings[0].clone();
        missing.location.module.source = directory.join("missing.luau");
        assert_eq!(
            diagnostics([missing], resolve).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        assert_eq!(diagnostics(findings, resolve).unwrap(), reports);
        assert_eq!(diagnostics([], resolve).unwrap(), BTreeMap::new());
    }
}
