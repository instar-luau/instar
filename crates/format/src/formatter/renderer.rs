use super::{
    builder::Plan,
    document::{Document, Layout},
};

use crate::{
    Result as Output,
    configuration::{Configuration, IndentStyle, LineEnding},
};

use instar_analysis::{Completion, Options, Reason};
use std::time::Instant;
use unicode_width::UnicodeWidthStr;

use vermis::{
    emitter::{Emitter, LineEnding as EmittedLineEnding},
    token::{Keyword, Symbol, TokenKind},
    tree::{TokenIndex, Tree},
};

pub(super) fn emit(
    tree: &Tree<'_>,
    configuration: &Configuration,
    plan: &Plan,
    options: &Options,
    started: Instant,
) -> Output {
    let line_ending = match configuration.line_ending {
        LineEnding::Lf => EmittedLineEnding::LineFeed,
        LineEnding::Crlf => EmittedLineEnding::CarriageReturnLineFeed,
    };

    let mut sources = Vec::new();

    for quote in &plan.quotes {
        if let Some(reason) = options.interrupted(started) {
            return Output::interrupted(reason);
        }

        sources.push(quote.as_ref().map(|quote| {
            let mut emitter = Emitter::new(line_ending);
            emitter.keyword(Keyword::Return);
            emitter.spaces(1);
            let mut source = emitter.finish().into_owned();
            source.extend_from_slice(quote);

            source
        }));
    }

    let mut replacements = Vec::new();

    for source in &sources {
        if let Some(reason) = options.interrupted(started) {
            return Output::interrupted(reason);
        }

        let Some(source) = source else {
            replacements.push(None);
            continue;
        };

        let replacement = vermis::parse(source);

        if !replacement.diagnostics.is_empty() {
            return Output::failed("quoted-string transformation produced invalid syntax");
        }

        let Some(token) = replacement
            .tokens
            .iter()
            .position(|token| token.kind == TokenKind::QuotedString)
        else {
            return Output::failed("quoted-string transformation did not produce a string token");
        };

        replacements.push(Some((replacement, TokenIndex::new(token))));
    }

    let mut renderer = Renderer {
        tree,
        configuration,
        options,
        started,
        interruption: None,
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

    if let Some(reason) = renderer
        .interruption
        .or_else(|| options.interrupted(started))
    {
        return Output::interrupted(reason);
    }

    if renderer.emitted {
        renderer
            .emitter
            .as_mut()
            .expect("output renderer")
            .newline();
    }

    Output {
        output: Some(
            renderer
                .emitter
                .expect("output renderer")
                .finish()
                .into_owned(),
        ),
        diagnostics: Vec::new(),
        completion: Completion::Complete,
    }
}

struct Renderer<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    options: &'tree Options,
    started: Instant,
    interruption: Option<Reason>,
    emitter: Option<Emitter<'source>>,
    replacements: &'tree [Option<(Tree<'source>, TokenIndex)>],
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
            self.interruption = self
                .interruption
                .or_else(|| self.options.interrupted(self.started));

            if self.interruption.is_some() || self.exceeded {
                return;
            }

            match item {
                Document::Token(index) => self.token(*index),

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
            options: self.options,
            started: self.started,
            interruption: None,
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

        !probe.exceeded && probe.interruption.is_none()
    }

    fn token(&mut self, index: TokenIndex) {
        self.separate();

        let (tree, token) = self.replacements[index.get()]
            .as_ref()
            .map_or((self.tree, index), |(tree, token)| (tree, *token));

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

    fn separate(&mut self) {
        if self.lines > 0 || !self.emitted {
            if let Some(emitter) = &mut self.emitter {
                if self.emitted {
                    for _ in 0..self.lines {
                        emitter.newline();
                    }
                }

                match self.configuration.indent_style {
                    IndentStyle::Tabs => emitter.tabs(self.indentation),

                    IndentStyle::Spaces => {
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
