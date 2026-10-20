use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use vermis::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::TokenKind,
    tree::{TokenIndex, Tree},
};

use super::{
    document::{Document, Layout, Nesting},
    plan::Plan,
};

use crate::{
    format::{Error, engine::parse, preservation},
    settings::format::{Configuration, Indentation, LineEnding},
};

struct Replacement<'source> {
    tree: Tree<'source>,
    token: TokenIndex,
}

struct Continuation<'document> {
    document: &'document [Document],
    mode: Option<Layout>,
    next: Option<&'document Self>,
}

struct Renderer<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    emitter: Option<Emitter<'source>>,
    replacements: &'tree [Option<Box<Replacement<'source>>>],
    placements: Vec<(TokenIndex, usize)>,
    indentation: usize,
    column: usize,
    lines: usize,
    has_pending_space: bool,
    has_emitted: bool,
    measurement: Option<Layout>,
    has_exceeded_width: bool,
}

impl Renderer<'_, '_> {
    fn render(
        &mut self,
        document: &[Document],
        mode: Option<Layout>,
        continuation: Option<&Continuation<'_>>,
    ) {
        let is_flat = mode.is_some();
        let is_continued = self.lines == 0 && self.has_emitted;

        for (offset, item) in document.iter().enumerate() {
            if self.has_exceeded_width {
                return;
            }

            let next = Continuation {
                document: &document[offset + 1..],
                mode,
                next: continuation,
            };

            match item {
                Document::Token(index) => self.token(*index),
                Document::Verbatim(range) => self.verbatim(range.clone()),

                Document::Symbol(symbol) => {
                    self.separate();

                    if let Some(emitter) = &mut self.emitter {
                        emitter.symbol(*symbol);
                    }

                    self.column += 1;
                    self.has_emitted = true;

                    self.has_exceeded_width |=
                        self.measurement.is_some() && self.column > self.configuration.width.get();
                }

                Document::Space => self.has_pending_space = true,

                Document::Line(lines) => {
                    self.lines = self.lines.max(*lines);
                    self.has_pending_space = false;
                    self.has_exceeded_width |= matches!(self.measurement, Some(Layout::Fit));
                }

                Document::Soft(spaces) => {
                    if is_flat {
                        self.has_pending_space |= *spaces > 0;
                    } else {
                        self.lines = self.lines.max(1);
                        self.has_pending_space = false;
                    }
                }

                Document::Indent { nesting, content } => {
                    let increment = usize::from(match nesting {
                        Nesting::Always => true,
                        Nesting::Broken => !is_flat,
                        Nesting::Continuation => !is_flat && is_continued,
                    });

                    let mode = if matches!(nesting, Nesting::Always) {
                        None
                    } else {
                        mode
                    };

                    self.indentation += increment;
                    self.render(content, mode, Some(&next));

                    self.indentation -= increment;
                }

                Document::Group { layout, content } => {
                    let column = if self.lines > 0 || !self.has_emitted {
                        self.indentation * self.configuration.indent_width.get()
                    } else {
                        self.column + usize::from(self.has_pending_space)
                    };

                    let selected = if matches!(layout, Layout::Vertical) {
                        None
                    } else if matches!(mode, Some(Layout::Arguments))
                        && matches!(layout, Layout::Arguments)
                    {
                        Some(Layout::Arguments)
                    } else if matches!(mode, Some(Layout::Fit))
                        || self.can_fit(content, column, Layout::Fit, Some(&next))
                    {
                        Some(Layout::Fit)
                    } else if matches!(layout, Layout::Arguments)
                        && self.can_fit(content, column, Layout::Arguments, Some(&next))
                    {
                        Some(Layout::Arguments)
                    } else {
                        None
                    };

                    self.has_exceeded_width |= selected.is_none()
                        && (matches!(self.measurement, Some(Layout::Fit))
                            || (matches!(self.measurement, Some(Layout::Arguments))
                                && matches!(mode, Some(Layout::Arguments))
                                && matches!(layout, Layout::Fit)));

                    self.render(content, selected, Some(&next));
                }

                Document::Choice(choice) => {
                    let column = if self.lines > 0 || !self.has_emitted {
                        self.indentation * self.configuration.indent_width.get()
                    } else {
                        self.column + usize::from(self.has_pending_space)
                    };

                    let layout = if choice.has_prefix {
                        Layout::Prefix
                    } else {
                        Layout::Fit
                    };

                    let selected = if matches!(mode, Some(Layout::Fit))
                        || self.can_fit(&choice.probe, column, layout, None)
                    {
                        &choice.inline
                    } else {
                        &choice.expanded
                    };

                    self.render(selected, mode, Some(&next));
                }
            }
        }
    }

    fn can_fit(
        &self,
        document: &[Document],
        column: usize,
        layout: Layout,
        mut continuation: Option<&Continuation<'_>>,
    ) -> bool {
        let mut probe = Renderer {
            tree: self.tree,
            configuration: self.configuration,
            emitter: None,
            replacements: self.replacements,
            placements: Vec::new(),
            indentation: self.indentation,
            column,
            lines: self.lines,
            has_pending_space: false,
            has_emitted: self.has_emitted,
            measurement: Some(layout),
            has_exceeded_width: false,
        };

        if matches!(layout, Layout::Prefix) {
            probe.measure_suffix(document, Some(Layout::Fit));
        } else {
            probe.render(document, Some(layout), continuation);
        }

        probe.measurement = None;

        while let Some(next) = continuation {
            if !probe.measure_suffix(next.document, next.mode) {
                break;
            }

            continuation = next.next;
        }

        !probe.has_exceeded_width
    }

    fn measure_suffix(&mut self, document: &[Document], mode: Option<Layout>) -> bool {
        for item in document {
            if self.has_exceeded_width || self.lines > 0 {
                return false;
            }

            match item {
                Document::Token(index) => {
                    let (tree, token) = self.replacements[index.get()]
                        .as_deref()
                        .map_or((self.tree, *index), |replacement| {
                            (&replacement.tree, replacement.token)
                        });

                    if tree.token(token).kind == TokenKind::Comment {
                        return false;
                    }

                    let bytes = tree.token(token).bytes(tree.source);

                    if matches!(self.measurement, Some(Layout::Prefix))
                        && matches!(
                            tree.token(token).kind,
                            TokenKind::QuotedString | TokenKind::RawString
                        )
                        && columns(bytes, 0, self.configuration.indent_width.get()).1
                            > self.configuration.width.get()
                    {
                        return false;
                    }

                    if tree.token(token).kind == TokenKind::BlockComment {
                        if bytes.contains(&b'\n') {
                            return false;
                        }

                        self.separate();
                    } else {
                        self.leading(bytes);
                    }
                }

                Document::Verbatim(range) => {
                    let start = self.tree.tokens[range.start].span.start;
                    let end = self.tree.tokens[range.end - 1].span.end;
                    self.leading(&self.tree.source[start..end]);
                }

                Document::Symbol(_) => {
                    self.separate();
                    self.column += 1;
                    self.has_emitted = true;
                    self.has_exceeded_width |= self.column > self.configuration.width.get();
                }

                Document::Space => self.has_pending_space = true,
                Document::Line(_) | Document::Choice(_) => return false,

                Document::Soft(spaces) => {
                    if mode.is_none() {
                        return false;
                    }

                    self.has_pending_space |= *spaces > 0;
                }

                Document::Indent { nesting, content } => {
                    let mode = if matches!(nesting, Nesting::Always) {
                        None
                    } else {
                        mode
                    };

                    if !self.measure_suffix(content, mode) {
                        return false;
                    }
                }

                Document::Group { content, .. } => {
                    if !self.measure_suffix(content, None) {
                        return false;
                    }
                }
            }
        }

        !self.has_exceeded_width && self.lines == 0
    }

    fn leading(&mut self, bytes: &[u8]) {
        self.separate();

        let end = bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .unwrap_or(bytes.len());

        let (column, maximum) = columns(
            &bytes[..end],
            self.column,
            self.configuration.indent_width.get(),
        );

        self.column = column;
        self.has_exceeded_width |= maximum > self.configuration.width.get();
        self.has_emitted = true;
        self.lines = usize::from(end < bytes.len());
    }

    fn token(&mut self, index: TokenIndex) {
        if self.measurement.is_none()
            && self.emitter.is_none()
            && (self.lines > 0 || !self.has_emitted)
            && self.tree.token(index).kind == TokenKind::BlockComment
        {
            self.placements.push((
                index,
                self.indentation * self.configuration.indent_width.get(),
            ));
        }

        self.separate();

        let (tree, token) = self.replacements[index.get()]
            .as_deref()
            .map_or((self.tree, index), |replacement| {
                (&replacement.tree, replacement.token)
            });

        if let Some(emitter) = &mut self.emitter {
            emitter.token(tree, token);
        }

        let bytes = tree.token(token).bytes(tree.source);

        let is_comment = matches!(
            tree.token(token).kind,
            TokenKind::Comment | TokenKind::BlockComment
        );

        let (column, maximum) = columns(bytes, self.column, self.configuration.indent_width.get());

        if !is_comment {
            self.column = column;
        } else if bytes.contains(&b'\n') {
            self.column = 0;
        }

        self.has_exceeded_width |= self.measurement.is_some()
            && ((!is_comment && maximum > self.configuration.width.get())
                || (matches!(self.measurement, Some(Layout::Fit)) && bytes.contains(&b'\n')));

        self.has_emitted = true;

        if tree.token(token).kind == TokenKind::Comment {
            self.lines = self.lines.max(1);
            self.has_pending_space = false;
            self.has_exceeded_width |= matches!(self.measurement, Some(Layout::Fit));
        }
    }

    fn verbatim(&mut self, range: Range<usize>) {
        let start = self.tree.tokens[range.start].span.start;

        let beginning = self.tree.source[..start]
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map_or(0, |position| position + 1);

        let prefix = &self.tree.source[beginning..start];

        if (self.lines > 0 || !self.has_emitted)
            && prefix.iter().all(|byte| matches!(byte, b' ' | b'\t'))
        {
            let indentation = self.indentation;
            self.indentation = 0;
            self.separate();
            self.indentation = indentation;

            if let Some(emitter) = &mut self.emitter {
                for byte in prefix {
                    if *byte == b'\t' {
                        emitter.tabs(1);
                    } else {
                        emitter.spaces(1);
                    }
                }
            }

            self.column = columns(prefix, 0, self.configuration.indent_width.get()).0;
        } else {
            self.separate();
        }

        if let Some(emitter) = &mut self.emitter {
            for position in range.clone() {
                emitter.token(self.tree, TokenIndex::new(position));
            }
        }

        let end = self.tree.tokens[range.end - 1].span.end;
        let bytes = &self.tree.source[start..end];
        let (column, maximum) = columns(bytes, self.column, self.configuration.indent_width.get());
        self.column = column;

        self.has_exceeded_width |= self.measurement.is_some()
            && (maximum > self.configuration.width.get()
                || matches!(self.measurement, Some(Layout::Fit)) && bytes.contains(&b'\n'));

        self.has_emitted = true;
    }

    fn separate(&mut self) {
        if self.lines > 0 || !self.has_emitted {
            if let Some(emitter) = &mut self.emitter {
                if self.has_emitted {
                    for _ in 0..self.lines {
                        emitter.newline();
                    }
                }

                match self.configuration.indent_style {
                    Indentation::Tabs => emitter.tabs(self.indentation),

                    Indentation::Spaces => {
                        emitter.spaces(self.indentation * self.configuration.indent_width.get());
                    }
                }
            }

            self.column = self.indentation * self.configuration.indent_width.get();
        } else if self.has_pending_space {
            if let Some(emitter) = &mut self.emitter {
                emitter.spaces(1);
            }

            self.column += 1;
        }

        self.lines = 0;
        self.has_pending_space = false;
    }
}

pub(crate) fn emit(
    tree: &Tree<'_>,
    configuration: &Configuration,
    plan: &Plan,
) -> Result<Vec<u8>, Error> {
    let line_ending = match configuration.line_ending {
        LineEnding::LineFeed => EmittedLineEnding::LineFeed,
        LineEnding::CarriageReturnLineFeed => EmittedLineEnding::CarriageReturnLineFeed,
    };

    let mut sources: Vec<_> = plan
        .quotes
        .iter()
        .map(|quote| {
            quote.as_ref().map(|quote| {
                let mut source = b"return ".to_vec();
                source.extend_from_slice(quote);

                source
            })
        })
        .collect();

    if tree.tokens.iter().any(|token| {
        token.kind == TokenKind::BlockComment && token.bytes(tree.source).contains(&b'\n')
    }) {
        let placements = {
            let replacements = replacements(&sources)?;

            let mut renderer = Renderer {
                tree,
                configuration,
                emitter: None,
                replacements: &replacements,
                placements: Vec::new(),
                indentation: 0,
                column: 0,
                lines: 0,
                has_pending_space: false,
                has_emitted: false,
                measurement: None,
                has_exceeded_width: false,
            };

            renderer.render(&plan.document, None, None);

            renderer.placements
        };

        for (index, columns) in placements {
            if let Some(source) = preservation::reindent(tree, index, columns, configuration) {
                sources[index.get()] = Some(source);
            }
        }
    }

    let replacements = replacements(&sources)?;

    let mut renderer = Renderer {
        tree,
        configuration,
        emitter: Some(Emitter::from_tree(tree, line_ending)),
        replacements: &replacements,
        placements: Vec::new(),
        indentation: 0,
        column: 0,
        lines: 0,
        has_pending_space: false,
        has_emitted: false,
        measurement: None,
        has_exceeded_width: false,
    };

    renderer.render(&plan.document, None, None);

    if renderer.has_emitted && plan.should_emit_final_newline {
        renderer
            .emitter
            .as_mut()
            .expect("Internal error: missing output renderer.")
            .newline();
    }

    Ok(renderer
        .emitter
        .expect("Internal error: missing output renderer.")
        .finish()
        .into_owned())
}

fn replacements(sources: &[Option<Vec<u8>>]) -> Result<Vec<Option<Box<Replacement<'_>>>>, Error> {
    sources
        .iter()
        .map(|source| {
            source
                .as_ref()
                .map(|source| {
                    let tree =
                        parse(source).map_err(|error| Error::internal(0..0, error.message))?;

                    let (kind, start) = if source.starts_with(b"return ") {
                        (TokenKind::QuotedString, 7)
                    } else {
                        (TokenKind::BlockComment, 0)
                    };

                    let token = tree
                        .tokens
                        .iter()
                        .position(|token| {
                            token.kind == kind
                                && token.span.start == start
                                && token.span.end == source.len()
                        })
                        .ok_or_else(|| {
                            Error::internal(0..0, "Formatted replacement has no complete token.")
                        })?;

                    Ok(Box::new(Replacement {
                        tree,
                        token: TokenIndex::new(token),
                    }))
                })
                .transpose()
        })
        .collect()
}

fn columns(bytes: &[u8], mut column: usize, tab_width: usize) -> (usize, usize) {
    if bytes.iter().all(|byte| (b' '..=b'~').contains(byte)) {
        let column = column + bytes.len();

        return (column, column);
    }

    let mut maximum = column;

    for segment in bytes.split_inclusive(|byte| matches!(byte, b'\n' | b'\t')) {
        let (text, separator) = match segment.split_last() {
            Some((&separator @ (b'\n' | b'\t'), text)) => (text, Some(separator)),
            _ => (segment, None),
        };

        column += UnicodeWidthStr::width(String::from_utf8_lossy(text).as_ref());
        maximum = maximum.max(column);

        match separator {
            Some(b'\n') => column = 0,

            Some(b'\t') => {
                column += tab_width - column % tab_width;
                maximum = maximum.max(column);
            }

            _ => {}
        }
    }

    (column, maximum)
}
