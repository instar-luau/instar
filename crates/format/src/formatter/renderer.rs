use super::{
    builder::Plan,
    document::{Document, Layout, width},
};

use crate::{
    Result as Output,
    configuration::{Configuration, IndentStyle, LineEnding},
};

use instar_analysis::{Completion, Options, Reason};
use std::time::Instant;

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
        quotes: &plan.quotes,
        options,
        started,
        interruption: None,
        emitter: Emitter::from_tree(tree, line_ending),
        replacements: &replacements,
        indentation: 0,
        column: 0,
        lines: 0,
        space: false,
        emitted: false,
    };

    renderer.render(&plan.document, false);

    if let Some(reason) = renderer
        .interruption
        .or_else(|| options.interrupted(started))
    {
        return Output::interrupted(reason);
    }

    if renderer.emitted {
        renderer.emitter.newline();
    }

    Output {
        output: Some(renderer.emitter.finish().into_owned()),
        diagnostics: Vec::new(),
        completion: Completion::Complete,
    }
}

struct Renderer<'tree, 'source> {
    tree: &'tree Tree<'source>,
    configuration: &'tree Configuration,
    quotes: &'tree [Option<Vec<u8>>],
    options: &'tree Options,
    started: Instant,
    interruption: Option<Reason>,
    emitter: Emitter<'source>,
    replacements: &'tree [Option<(Tree<'source>, TokenIndex)>],
    indentation: usize,
    column: usize,
    lines: usize,
    space: bool,
    emitted: bool,
}

impl Renderer<'_, '_> {
    fn render(&mut self, document: &[Document], flat: bool) {
        for item in document {
            self.interruption = self
                .interruption
                .or_else(|| self.options.interrupted(self.started));

            if self.interruption.is_some() {
                return;
            }

            match item {
                Document::Token(index) => {
                    self.separate();

                    let bytes = if let Some((tree, token)) = &self.replacements[index.get()] {
                        self.emitter.token(tree, *token);

                        tree.token(*token).bytes(tree.source)
                    } else {
                        self.emitter.token(self.tree, *index);

                        self.tree.token(*index).bytes(self.tree.source)
                    };

                    self.column = bytes
                        .iter()
                        .rposition(|&byte| byte == b'\n')
                        .map_or(self.column + bytes.len(), |position| {
                            bytes.len() - position - 1
                        });

                    self.emitted = true;
                }

                Document::Symbol(symbol) => {
                    self.separate();
                    self.emitter.symbol(*symbol);
                    self.column += 1;
                    self.emitted = true;
                }

                Document::Space => self.space = true,

                Document::Line(lines) => {
                    self.lines = self.lines.max(*lines);
                    self.space = false;
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
                        self.emitter.symbol(Symbol::Comma);
                        self.column += 1;
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
                        Layout::Vertical => false,
                        Layout::Pressed => true,

                        Layout::Fit => {
                            flat || width(
                                content,
                                self.tree,
                                self.quotes,
                                self.options,
                                self.started,
                            )
                            .is_some_and(|width| column + width <= self.configuration.width.get())
                        }
                    };

                    self.render(content, flat);
                }
            }
        }
    }

    fn separate(&mut self) {
        if self.lines > 0 || !self.emitted {
            if self.emitted {
                for _ in 0..self.lines {
                    self.emitter.newline();
                }
            }

            match self.configuration.indent_style {
                IndentStyle::Tabs => self.emitter.tabs(self.indentation),

                IndentStyle::Spaces => self
                    .emitter
                    .spaces(self.indentation * self.configuration.indent_width.get()),
            }

            self.column = self.indentation * self.configuration.indent_width.get();
        } else if self.space {
            self.emitter.spaces(1);
            self.column += 1;
        }

        self.lines = 0;
        self.space = false;
    }
}
