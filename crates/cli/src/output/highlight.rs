use std::{collections::BTreeMap, sync::Arc};

use instar_core::analysis::{Token, TokenKind, analyze};
use miette::{NamedSource, SpanContents, highlighters};
use owo_colors::{Style, Styled};

pub(super) struct Source {
    tokens: Vec<Token>,
    lines: Vec<usize>,
}

impl Source {
    pub(super) fn new(source: impl Into<Arc<str>>) -> Self {
        let source = source.into();

        let lines = source
            .split_inclusive('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len();

                Some(start)
            })
            .collect();

        let tokens = analyze(source).tokens;

        Self { tokens, lines }
    }

    pub(super) fn span<'text>(
        &self,
        index: usize,
        offset: usize,
        text: &'text str,
        background: Style,
    ) -> Vec<Styled<&'text str>> {
        let Some(start) = self.lines.get(index).copied() else {
            return vec![background.style(text)];
        };

        let start = start + offset;
        let end = start + text.len();

        let first = self
            .tokens
            .partition_point(|token| token.range.end <= start);

        let mut result = Vec::new();
        let mut previous = 0;

        for token in self.tokens[first..]
            .iter()
            .take_while(|token| token.range.start < end)
        {
            let begin = token.range.start.max(start) - start;
            let finish = token.range.end.min(end) - start;

            if previous < begin {
                result.push(background.style(&text[previous..begin]));
            }

            result.push(style(token.kind, background).style(&text[begin..finish]));
            previous = finish;
        }

        if previous < text.len() {
            result.push(background.style(&text[previous..]));
        }

        result
    }
}

pub(super) struct Highlighter {
    sources: BTreeMap<String, Source>,
}

impl Highlighter {
    pub(super) fn new<'source>(
        sources: impl Iterator<Item = &'source NamedSource<Arc<str>>>,
    ) -> Self {
        Self {
            sources: sources
                .map(|source| {
                    let name = source.name().to_owned();
                    let source = Source::new(Arc::clone(source.inner()));

                    (name, source)
                })
                .collect(),
        }
    }
}

impl highlighters::Highlighter for Highlighter {
    fn start_highlighter_state<'source>(
        &'source self,
        source: &dyn SpanContents<'_>,
    ) -> Box<dyn highlighters::HighlighterState + 'source> {
        match source.name().and_then(|name| self.sources.get(name)) {
            Some(highlighting) => Box::new(State {
                source: highlighting,
                line: source.line(),
            }),

            None => Box::new(highlighters::BlankHighlighterState),
        }
    }
}

struct State<'source> {
    source: &'source Source,
    line: usize,
}

impl highlighters::HighlighterState for State<'_> {
    fn highlight_line<'line>(&mut self, line: &'line str) -> Vec<Styled<&'line str>> {
        let highlighted = self.source.span(self.line, 0, line, Style::new());
        self.line += 1;

        highlighted
    }
}

fn style(kind: TokenKind, background: Style) -> Style {
    match kind {
        TokenKind::Keyword => background.magenta(),
        TokenKind::String => background.green(),
        TokenKind::Number | TokenKind::Parameter => background.yellow(),
        TokenKind::Comment => background.dimmed(),
        TokenKind::Function | TokenKind::Method => background.blue(),
        TokenKind::Namespace => background.bright_blue(),
        TokenKind::Type | TokenKind::TypeParameter => background.cyan(),
        TokenKind::Property => background.bright_cyan(),
        TokenKind::Operator => background.bright_magenta(),
        TokenKind::Decorator => background.yellow().bold(),
        TokenKind::Variable => background,
    }
}
