use std::ops::Range;

use unicode_width::UnicodeWidthStr;

use vermis::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::{Symbol, TokenKind},
    tree::{TokenIndex, Tree},
};

use super::{
    document::{Document, Layout},
    plan::Plan,
};

use crate::{
    format::{Error, formatting::parse},
    settings::format::{Configuration, Indentation, LineEnding},
};

struct Replacement<'source> {
    tree: Tree<'source>,
    token: TokenIndex,
}

struct Renderer<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    emitter: Option<Emitter<'source>>,
    replacements: &'tree [Option<Box<Replacement<'source>>>],
    indentation: usize,
    column: usize,
    lines: usize,
    space: bool,
    emitted: bool,
    measurement: Option<Layout>,
    exceeded: bool,
}

impl Renderer<'_, '_> {
    fn render(&mut self, document: &[Document], flat: bool) {
        for item in document {
            if self.exceeded {
                return;
            }

            match item {
                Document::Token(index) => self.token(*index),
                Document::Verbatim(range) => self.verbatim(range.clone()),

                Document::Symbol(symbol) => {
                    self.separate();

                    if let Some(emitter) = &mut self.emitter {
                        emitter.symbol(*symbol);
                    }

                    self.column += 1;
                    self.emitted = true;

                    self.exceeded |=
                        self.emitter.is_none() && self.column > self.configuration.width.get();
                }

                Document::Space => self.space = true,

                Document::Line(lines) => {
                    self.lines = self.lines.max(*lines);
                    self.space = false;
                    self.exceeded |= matches!(self.measurement, Some(Layout::Fit));
                }

                Document::Soft(spaces) => {
                    if flat {
                        self.space |= *spaces > 0;
                    } else {
                        self.lines = self.lines.max(1);
                        self.space = false;
                    }
                }

                Document::TrailingComma => {
                    if !flat {
                        self.separate();

                        if let Some(emitter) = &mut self.emitter {
                            emitter.symbol(Symbol::Comma);
                        }

                        self.column += 1;

                        self.exceeded |=
                            self.emitter.is_none() && self.column > self.configuration.width.get();
                    }
                }

                Document::Indent {
                    conditional,
                    content,
                } => {
                    let increment = usize::from(!conditional || !flat);
                    self.indentation += increment;
                    self.render(content, flat);
                    self.indentation -= increment;
                }

                Document::Group { layout, content } => {
                    let column = if self.lines > 0 || !self.emitted {
                        self.indentation * self.configuration.indent_width.get()
                    } else {
                        self.column + usize::from(self.space)
                    };

                    let flat = match layout {
                        Layout::Vertical => {
                            self.exceeded |= matches!(self.measurement, Some(Layout::Fit));

                            false
                        }

                        Layout::Arguments | Layout::Fit => {
                            flat || self.fits(content, column, *layout)
                        }
                    };

                    self.render(content, flat);
                }
            }
        }
    }

    fn fits(&self, document: &[Document], column: usize, layout: Layout) -> bool {
        let mut probe = Renderer {
            tree: self.tree,
            configuration: self.configuration,
            emitter: None,
            replacements: self.replacements,
            indentation: self.indentation,
            column,
            lines: 0,
            space: false,
            emitted: true,
            measurement: Some(layout),
            exceeded: false,
        };

        probe.render(document, true);

        !probe.exceeded
    }

    fn token(&mut self, index: TokenIndex) {
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
        let (column, maximum) = columns(bytes, self.column, self.configuration.indent_width.get());
        self.column = column;

        self.exceeded |= self.emitter.is_none()
            && (maximum > self.configuration.width.get()
                || (matches!(self.measurement, Some(Layout::Fit)) && bytes.contains(&b'\n')));

        self.emitted = true;
    }

    fn verbatim(&mut self, range: Range<usize>) {
        self.separate();

        if let Some(emitter) = &mut self.emitter {
            for position in range.clone() {
                emitter.token(self.tree, TokenIndex::new(position));
            }
        }

        let start = self.tree.tokens[range.start].span.start;
        let end = self.tree.tokens[range.end - 1].span.end;
        let bytes = &self.tree.source[start..end];
        let (column, maximum) = columns(bytes, self.column, self.configuration.indent_width.get());
        self.column = column;

        self.exceeded |= self.emitter.is_none()
            && (maximum > self.configuration.width.get()
                || matches!(self.measurement, Some(Layout::Fit)) && bytes.contains(&b'\n'));

        self.emitted = true;
    }

    fn separate(&mut self) {
        if self.lines > 0 || !self.emitted {
            if let Some(emitter) = &mut self.emitter {
                if self.emitted {
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
        } else if self.space {
            if let Some(emitter) = &mut self.emitter {
                emitter.spaces(1);
            }

            self.column += 1;
        }

        self.lines = 0;
        self.space = false;
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

    let sources: Vec<_> = plan
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

    let replacements = sources
        .iter()
        .map(|source| {
            source
                .as_ref()
                .map(|source| {
                    let tree =
                        parse(source).map_err(|error| Error::internal(0..0, error.message))?;

                    let token = tree
                        .tokens
                        .iter()
                        .position(|token| token.kind == TokenKind::QuotedString)
                        .ok_or_else(|| {
                            Error::internal(0..0, "formatted string has no quoted token")
                        })?;

                    Ok(Box::new(Replacement {
                        tree,
                        token: TokenIndex::new(token),
                    }))
                })
                .transpose()
        })
        .collect::<Result<Vec<_>, Error>>()?;

    let mut renderer = Renderer {
        tree,
        configuration,
        emitter: Some(Emitter::from_tree(tree, line_ending)),
        replacements: &replacements,
        indentation: 0,
        column: 0,
        lines: 0,
        space: false,
        emitted: false,
        measurement: None,
        exceeded: false,
    };

    renderer.render(&plan.document, false);

    if renderer.emitted && plan.final_newline {
        renderer
            .emitter
            .as_mut()
            .expect("output renderer")
            .newline();
    }

    Ok(renderer
        .emitter
        .expect("output renderer")
        .finish()
        .into_owned())
}

fn columns(bytes: &[u8], mut column: usize, tab_width: usize) -> (usize, usize) {
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
