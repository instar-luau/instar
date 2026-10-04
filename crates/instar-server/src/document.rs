use std::{
    collections::BTreeMap,
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use tower_lsp_server::ls_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DocumentSymbol, FoldingRange,
    FoldingRangeKind, Location, Position, Range, SelectionRange, SemanticToken, SemanticTokens,
    SymbolKind, Uri,
};

use vermis::tree::{Node, NodeIndex, NodeKind, Tree};

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
    symbols: Vec<DocumentSymbol>,
    folds: Vec<FoldingRange>,
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
    findings: impl IntoIterator<Item = instar_core::diagnostic::Diagnostic>,
    mut document: impl FnMut(&Path) -> io::Result<Arc<Document>>,
) -> io::Result<BTreeMap<PathBuf, Vec<Diagnostic>>> {
    use instar_core::diagnostic::Severity;

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

            let mut syntax = Syntax {
                spans: Vec::new(),
                symbols: Vec::new(),
                folds: Vec::new(),
                bindings: Index::new(&tree),
                features: crate::features::Syntax::new(&tree),
            };

            let methods = tree
                .nodes
                .iter()
                .filter_map(|node| {
                    if let NodeKind::Class { ref members, .. } = node.kind {
                        Some(tree.list(members))
                    } else {
                        None
                    }
                })
                .flatten()
                .map(|entry| entry.node)
                .collect::<Vec<_>>();

            for (index, node) in tree.nodes.iter().enumerate() {
                let span = node.span;

                if span.end > self.text.len() || span.start > span.end {
                    continue;
                }

                syntax.spans.push((span.start, span.end));

                let token = match node.kind {
                    NodeKind::Unary { operator, .. }
                    | NodeKind::Binary { operator, .. }
                    | NodeKind::CompoundAssignment { operator, .. } => Some(operator),

                    NodeKind::Assignment { assignment, .. } => assignment,
                    NodeKind::FunctionName { colon, .. } => colon,

                    NodeKind::TypeTable { access, .. }
                    | NodeKind::TypeField { access, .. }
                    | NodeKind::TypeIndexer { access, .. } => access,

                    NodeKind::TypeOf { keyword, .. } => Some(keyword),
                    _ => None,
                };

                if let Some(token) = token {
                    let span = tree.token(token).span;
                    syntax.spans.push((span.start, span.end));
                }

                let range = self.range(span.start, span.end);

                let fold = match node.kind {
                    NodeKind::Function { prefix, .. } => {
                        prefix.is_none_or(|prefix| {
                            matches!(tree.token(prefix).bytes(tree.source), b"local" | b"const")
                        }) && !methods.contains(&NodeIndex::new(index))
                    }

                    NodeKind::If { .. }
                    | NodeKind::While { .. }
                    | NodeKind::Repeat { .. }
                    | NodeKind::NumericFor { .. }
                    | NodeKind::GenericFor { .. }
                    | NodeKind::Do { .. }
                    | NodeKind::Table { .. }
                    | NodeKind::TypeTable { .. }
                    | NodeKind::Class { .. }
                    | NodeKind::String { .. } => true,

                    _ => false,
                };

                if range.end.line > range.start.line && fold {
                    syntax.folds.push(FoldingRange {
                        start_line: range.start.line,
                        start_character: Some(range.start.character),
                        end_line: range.end.line,
                        end_character: Some(range.end.character),
                        kind: Some(FoldingRangeKind::Region),
                        collapsed_text: None,
                    });
                }

                self.collect_symbols(&tree, node, &mut syntax.symbols);
            }

            syntax.spans.sort_unstable();
            syntax.spans.dedup();

            syntax
        })
    }

    fn collect_symbols(&self, tree: &Tree<'_>, node: &Node, symbols: &mut Vec<DocumentSymbol>) {
        #[expect(
            deprecated,
            reason = "the optional LSP compatibility field remains unset"
        )]
        let mut symbol = |name: NodeIndex, kind: SymbolKind| {
            let selection = tree.node(name).span;

            symbols.push(DocumentSymbol {
                name: String::from_utf8_lossy(tree.text(name)).into_owned(),
                detail: None,
                kind,
                tags: None,
                deprecated: None,
                range: self.range(node.span.start, node.span.end),
                selection_range: self.range(selection.start, selection.end),
                children: None,
            });
        };

        match &node.kind {
            NodeKind::Function {
                name: Some(name), ..
            } => {
                symbol(*name, SymbolKind::FUNCTION);
            }

            NodeKind::Local {
                bindings, values, ..
            }
            | NodeKind::Constant {
                bindings, values, ..
            } => {
                let values = tree.list(values);

                for (i, binding) in tree.list(bindings).iter().enumerate() {
                    if let NodeKind::Binding { name, .. } = tree.node(binding.node).kind {
                        let kind = if values.get(i).is_some_and(|value| {
                            matches!(tree.node(value.node).kind, NodeKind::Function { .. })
                        }) {
                            SymbolKind::FUNCTION
                        } else {
                            SymbolKind::VARIABLE
                        };

                        symbol(name, kind);
                    }
                }
            }

            NodeKind::TypeAlias { name, .. } | NodeKind::Class { name, .. } => {
                symbol(*name, SymbolKind::CLASS);
            }

            _ => {}
        }
    }

    pub(crate) fn bindings(&self) -> &Index {
        &self.syntax().bindings
    }

    pub(crate) fn features(&self) -> &crate::features::Syntax {
        &self.syntax().features
    }

    pub(crate) fn tokens(&self) -> SemanticTokens {
        let mut data = Vec::<SemanticToken>::new();
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
                    data.push(SemanticToken {
                        delta_line: line - previous.0,
                        delta_start: if line == previous.0 {
                            start - previous.1
                        } else {
                            start
                        },
                        length: end - start,
                        token_type: kind as u32,
                        token_modifiers_bitset: modifiers,
                    });

                    previous = (line, start);
                }
            }
        }

        SemanticTokens {
            result_id: None,
            data,
        }
    }

    pub(crate) fn symbols(&self) -> &[DocumentSymbol] {
        &self.syntax().symbols
    }

    pub(crate) fn folds(&self) -> &[FoldingRange] {
        &self.syntax().folds
    }

    pub(crate) fn selections(&self, positions: &[Position]) -> io::Result<Vec<SelectionRange>> {
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
            let mut parent = None;
            let mut enclosing = (0, self.text.len());

            for (start, end) in spans {
                if start < enclosing.0 || end > enclosing.1 {
                    continue;
                }

                parent = Some(SelectionRange {
                    range: self.range(start, end),
                    parent: parent.map(Box::new),
                });

                enclosing = (start, end);
            }

            selections.push(parent.unwrap_or_else(|| SelectionRange {
                range: self.range(offset, offset),
                parent: None,
            }));
        }

        Ok(selections)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use instar_core::{
        diagnostic::{Diagnostic as Finding, Location as SourceLocation, Severity},
        resolve::Module,
    };

    #[test]
    fn typed_tokens_preserve_utf16_and_multiline_deltas() {
        let document = Document::new(
            uri(&std::env::temp_dir().join("tokens.luau")).unwrap(),
            1,
            "local value = [[𐐀a\r\n\r\nb]]\r\nreturn value\n".into(),
        )
        .unwrap();

        let tokens = document.tokens();
        assert_eq!(tokens.result_id, None);

        let strings = tokens
            .data
            .iter()
            .filter(|token| token.token_type == crate::bindings::SemanticKind::String as u32)
            .map(|token| {
                (
                    token.delta_line,
                    token.delta_start,
                    token.length,
                    token.token_modifiers_bitset,
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(strings, vec![(0, 8, 5, 0), (2, 0, 3, 0)]);
        let mut position = Position::new(0, 0);
        let mut ranges = Vec::new();

        for token in tokens.data {
            position.line += token.delta_line;

            position.character = if token.delta_line == 0 {
                position.character + token.delta_start
            } else {
                token.delta_start
            };

            ranges.push(Range::new(
                position,
                Position::new(position.line, position.character + token.length),
            ));
        }

        assert!(ranges.contains(&Range::new(Position::new(3, 0), Position::new(3, 6))));
        assert!(ranges.contains(&Range::new(Position::new(3, 7), Position::new(3, 12))));
        assert!(document.selections(&[Position::new(0, 17)]).is_err());
    }

    #[test]
    fn typed_syntax_ranges_keep_symbols_folds_and_selection_parents() {
        let document = Document::new(
            uri(&std::env::temp_dir().join("syntax.luau")).unwrap(),
            1,
            "local function outer()\n    local value = (1 + 2)\n    return value\nend\n".into(),
        )
        .unwrap();

        let symbols = document.symbols();

        let outer = symbols
            .iter()
            .find(|symbol| symbol.name == "outer")
            .unwrap();

        assert_eq!(outer.kind, SymbolKind::FUNCTION);

        assert_eq!(
            outer.selection_range,
            Range::new(Position::new(0, 15), Position::new(0, 20))
        );

        let value = symbols
            .iter()
            .find(|symbol| symbol.name == "value")
            .unwrap();

        assert_eq!(value.kind, SymbolKind::VARIABLE);

        assert_eq!(
            value.selection_range,
            Range::new(Position::new(1, 10), Position::new(1, 15))
        );

        assert!(document.folds().iter().any(|fold| {
            fold.start_line == 0
                && fold.end_line == 3
                && fold.kind == Some(FoldingRangeKind::Region)
        }));

        let position = Position::new(1, 19);
        let selections = document.selections(&[position]).unwrap();
        assert_eq!(selections.len(), 1);
        let mut selection = &selections[0];
        assert_eq!(selection.range, Range::new(position, Position::new(1, 20)));
        let group = Range::new(Position::new(1, 18), Position::new(1, 25));
        let mut saw_group = false;
        let mut saw_function = false;

        while let Some(parent) = selection.parent.as_deref() {
            assert!(parent.range.start <= selection.range.start);
            assert!(parent.range.end >= selection.range.end);
            assert_ne!(parent.range, selection.range);
            saw_group |= parent.range == group;
            saw_function |= parent.range == outer.range;
            selection = parent;
        }

        assert!(saw_group);
        assert!(saw_function);

        let empty = Document::new(
            uri(&std::env::temp_dir().join("empty.luau")).unwrap(),
            1,
            String::new(),
        )
        .unwrap();

        assert_eq!(
            empty.selections(&[Position::new(0, 0)]).unwrap(),
            vec![SelectionRange {
                range: Range::new(Position::new(0, 0), Position::new(0, 0)),
                parent: None,
            }]
        );
    }

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

        assert_eq!(diagnostics([], resolve).unwrap(), BTreeMap::new());
    }
}
