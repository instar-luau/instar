use std::{collections::BTreeSet, sync::Arc};

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, Language};
use text_size::{TextRange, TextSize};

use crate::source::{Source, SourceError};

use super::lexer::{self, Lexeme, Token};
use super::{Luau, SyntaxKind, SyntaxNode, SyntaxToken};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub range: TextRange,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Feature {
    Classes,
    ConditionalBindings,
    IntegerLiterals,
    ValueExports,
    DebugNoInline,
    Declarations,
}

#[derive(Clone, Debug, Default)]
pub struct ParseOptions {
    pub features: BTreeSet<Feature>,

    /// Omitted uses `LuauRecursionLimit` (1000).
    pub recursion_limit: Option<usize>,

    /// Omitted uses `LuauTypeLengthLimit` (1000).
    pub type_length_limit: Option<usize>,

    /// Omitted uses `LuauParseErrorLimit` (100).
    pub error_limit: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct FeatureUse {
    pub feature: Feature,
    pub range: TextRange,
}

#[derive(Clone, Debug)]
pub struct HotComment {
    pub range: TextRange,
    pub header: bool,
    pub text: Vec<u8>,
}

#[derive(Clone, Copy, Debug)]
pub enum EntryPoint {
    Module,
    Expression,
    Type,
}

#[derive(Clone, Debug)]
pub struct Parse {
    source: Arc<Source>,
    green: GreenNode,
    errors: Vec<ParseError>,
    options: ParseOptions,
    feature_uses: Vec<FeatureUse>,
    hot_comments: Vec<HotComment>,
    extent: TextRange,
}

#[derive(Clone, Copy)]
enum Expression {
    Value,
    Variable,
    Call,
    Vararg,
}

impl Expression {
    fn is_variable(self) -> bool {
        matches!(self, Self::Variable)
    }

    fn is_multiple(self) -> bool {
        matches!(self, Self::Call | Self::Vararg)
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TypeForm {
    Type,
    Pack,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TypeContext {
    Type,
    Pack,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum StatementFlow {
    Continue,
    Terminal,
}

struct Parser<'source, 'options> {
    text: &'source str,
    bytes: &'source [u8],
    tokens: Vec<Token>,
    cursor: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
    options: &'options ParseOptions,
    feature_uses: Vec<FeatureUse>,
    fatal: bool,
    depth: usize,
    blocks: usize,
    loops: usize,
    vararg: bool,
    declaration_context: bool,
    error_kind: SyntaxKind,
    openers: Vec<(Lexeme, usize)>,
}

impl Parse {
    /// # Errors
    /// Rejects source ranges that cannot be represented.
    pub fn new(source: Arc<Source>) -> Result<Self, SourceError> {
        Self::with_options(source, ParseOptions::default())
    }

    /// # Errors
    /// Rejects source ranges that cannot be represented.
    pub fn with_options(source: Arc<Source>, options: ParseOptions) -> Result<Self, SourceError> {
        Self::entry(source, options, EntryPoint::Module)
    }

    /// # Errors
    /// Rejects source ranges that cannot be represented.
    pub fn entry(
        source: Arc<Source>,
        options: ParseOptions,
        entry: EntryPoint,
    ) -> Result<Self, SourceError> {
        let extent = range(0, source.bytes().len());

        Self::fragment(source, extent, options, entry)
    }

    /// Fragment syntax ranges are relative; diagnostics refer to the original source.
    /// # Errors
    /// Rejects out-of-bounds ranges and boundaries inside a UTF-8 scalar.
    pub fn fragment(
        source: Arc<Source>,
        extent: TextRange,
        options: ParseOptions,
        entry: EntryPoint,
    ) -> Result<Self, SourceError> {
        let view = source.syntax_text();
        let text = view
            .get(usize::from(extent.start())..usize::from(extent.end()))
            .ok_or(SourceError::Range)?;

        let bytes = source.slice(extent)?;
        let (tokens, lexical_errors) = lexer::lex(bytes);
        let mut hot_comments =
            collect_hot_comments(bytes, &tokens, extent.start() == TextSize::from(0));

        let mut parser = Parser {
            text,
            bytes,
            tokens,
            cursor: 0,
            builder: GreenNodeBuilder::new(),
            errors: lexical_errors
                .into_iter()
                .map(|error| ParseError {
                    range: range(error.range.start, error.range.end),
                    message: error.message.into(),
                })
                .collect(),
            options: &options,
            feature_uses: Vec::new(),
            fatal: false,
            depth: 0,
            blocks: 0,
            loops: 0,
            vararg: true,
            declaration_context: false,
            error_kind: SyntaxKind::ErrorStatement,
            openers: Vec::new(),
        };

        parser.start(SyntaxKind::Root);

        match entry {
            EntryPoint::Module => parser.parse_block(&[]),
            EntryPoint::Expression => {
                parser.parse_expression(0);
            }
            EntryPoint::Type => parser.parse_type(),
        }

        if !parser.eof() {
            parser.consume_remainder("expected end of input");
        }

        parser.trivia();
        parser.finish();

        let green = parser.builder.finish();
        let mut errors = parser.errors;
        let mut feature_uses = parser.feature_uses;

        for error in &mut errors {
            error.range += extent.start();
        }

        for usage in &mut feature_uses {
            usage.range += extent.start();
        }

        for comment in &mut hot_comments {
            comment.range += extent.start();
        }

        drop(view);

        Ok(Self {
            source,
            green,
            errors,
            options,
            feature_uses,
            hot_comments,
            extent,
        })
    }

    #[must_use]
    pub fn source(&self) -> &Arc<Source> {
        &self.source
    }

    #[must_use]
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    #[must_use]
    pub const fn options(&self) -> &ParseOptions {
        &self.options
    }

    #[must_use]
    pub fn feature_uses(&self) -> &[FeatureUse] {
        &self.feature_uses
    }

    #[must_use]
    pub fn hot_comments(&self) -> &[HotComment] {
        &self.hot_comments
    }

    #[must_use]
    pub const fn extent(&self) -> TextRange {
        self.extent
    }

    #[must_use]
    pub fn source_range(&self, relative: TextRange) -> TextRange {
        relative + self.extent.start()
    }

    #[must_use]
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }
}

impl Parser<'_, '_> {
    // Token navigation
    fn nth(&self, offset: usize) -> Option<Token> {
        self.tokens[self.cursor..]
            .iter()
            .copied()
            .filter(|token| !token.kind.is_trivia())
            .nth(offset)
    }

    fn current(&self) -> Option<Token> {
        self.nth(0)
    }

    fn lexeme(&self) -> Option<Lexeme> {
        self.current().map(|token| token.lexeme)
    }

    fn kind(&self) -> Option<SyntaxKind> {
        self.current().map(|token| token.kind)
    }

    fn at(&self, lexeme: Lexeme) -> bool {
        self.lexeme() == Some(lexeme)
    }

    fn nth_at(&self, offset: usize, lexeme: Lexeme) -> bool {
        self.nth(offset).is_some_and(|token| token.lexeme == lexeme)
    }

    fn at_any(&self, lexemes: &[Lexeme]) -> bool {
        self.lexeme()
            .is_some_and(|lexeme| lexemes.contains(&lexeme))
    }

    fn eof(&self) -> bool {
        self.current().is_none()
    }

    fn start_offset(&self) -> usize {
        self.current().map_or(self.bytes.len(), |token| token.start)
    }

    fn spelling(&self) -> &str {
        self.current()
            .map_or("", |token| &self.text[token.start..token.end])
    }

    fn previous_end(&self) -> usize {
        self.tokens[..self.cursor]
            .iter()
            .rev()
            .find(|token| !token.kind.is_trivia())
            .map_or(0, |token| token.end)
    }

    fn eat(&mut self, lexeme: Lexeme) -> bool {
        if self.at(lexeme) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn bump(&mut self) {
        self.trivia();

        if let Some(token) = self.tokens.get(self.cursor) {
            let closing = match token.lexeme {
                Lexeme::OpenParen => Some(Lexeme::CloseParen),
                Lexeme::OpenBracket | Lexeme::AttributeOpen => Some(Lexeme::CloseBracket),
                Lexeme::OpenBrace => Some(Lexeme::CloseBrace),
                _ => None,
            };

            if let Some(closing) = closing {
                self.openers.push((closing, token.start));
            } else if self
                .openers
                .last()
                .is_some_and(|(closing, _)| *closing == token.lexeme)
            {
                self.openers.pop();
            }
        }

        self.raw_bump();
    }

    // Lossless tree construction
    fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(Luau::kind_to_raw(kind));
    }

    fn finish(&mut self) {
        self.builder.finish_node();
    }

    fn checkpoint(&mut self) -> Checkpoint {
        self.trivia();

        self.builder.checkpoint()
    }

    fn wrap(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder
            .start_node_at(checkpoint, Luau::kind_to_raw(kind));
    }

    fn emit(&mut self, kind: SyntaxKind, start: usize, end: usize) {
        self.builder
            .token(Luau::kind_to_raw(kind), &self.text[start..end]);
    }

    fn raw_bump(&mut self) {
        if let Some(token) = self.tokens.get(self.cursor).copied() {
            self.emit(token.kind, token.start, token.end);
            self.cursor += 1;
        }
    }

    fn trivia(&mut self) {
        while self
            .tokens
            .get(self.cursor)
            .is_some_and(|token| token.kind.is_trivia())
        {
            self.raw_bump();
        }
    }

    fn finish_statement(&mut self) {
        self.eat(Lexeme::Semicolon);
        self.finish();
    }

    // Diagnostics, recovery and resource limits
    fn report(&mut self, span: TextRange, message: &str) {
        if self.fatal || self.errors.last().is_some_and(|error| error.range == span) {
            return;
        }

        let limit = self.options.error_limit.unwrap_or(100);

        if self.errors.len() >= limit {
            if limit == 1 {
                self.errors.truncate(1);
            } else {
                self.errors.push(ParseError {
                    range: span,
                    message: format!("Reached error limit ({limit})"),
                });
                self.fatal = true;
            }

            return;
        }

        self.errors.push(ParseError {
            range: span,
            message: message.into(),
        });

        if limit > 1 && self.errors.len() >= limit {
            self.errors.push(ParseError {
                range: span,
                message: format!("Reached error limit ({limit})"),
            });
            self.fatal = true;
        }
    }

    fn error(&mut self, message: &str) {
        let span = self
            .current()
            .map_or(range(self.bytes.len(), self.bytes.len()), |token| {
                range(token.start, token.end)
            });

        self.error_range(span, message);
    }

    fn error_range(&mut self, span: TextRange, message: &str) {
        self.report(span, message);

        self.start(self.error_kind);
        self.start(SyntaxKind::Error);
        self.finish();
        self.finish();
    }

    fn expect(&mut self, expected: Lexeme) {
        if self.eat(expected) {
            return;
        }

        let actual = if self.eof() {
            "<eof>".into()
        } else {
            format!("'{}'", self.spelling())
        };

        let expected_text = lexeme_spelling(expected);

        let opener = self
            .openers
            .iter()
            .rev()
            .find(|(closing, _)| *closing == expected);

        let message = if let Some((_, start)) = opener {
            let line = self.text[..*start].matches('\n').count();
            let current_line = self.text[..self.start_offset()].matches('\n').count();

            let position = if line == current_line {
                let column = self.bytes[..*start]
                    .iter()
                    .rposition(|byte| *byte == b'\n')
                    .map_or(*start + 1, |newline| *start - newline);

                format!("column {column}")
            } else {
                format!("line {}", line + 1)
            };

            let opening_text = &self.text[*start..=*start];

            format!(
                "Expected '{expected_text}' (to close '{opening_text}' at {position}), got {actual}"
            )
        } else {
            format!("Expected '{expected_text}', got {actual}")
        };

        self.error(&message);

        if self.nth_at(1, expected) {
            self.recover_token();
            self.bump();
        }
    }

    fn recover_token(&mut self) {
        self.start(self.error_kind);
        self.start(SyntaxKind::Error);
        self.bump();
        self.finish();
        self.finish();
    }

    fn boundary(&self) -> bool {
        self.eof()
            || self.at_any(&[
                Lexeme::End,
                Lexeme::Else,
                Lexeme::ElseIf,
                Lexeme::Until,
                Lexeme::Then,
                Lexeme::Do,
                Lexeme::Local,
                Lexeme::Return,
                Lexeme::For,
                Lexeme::While,
                Lexeme::Repeat,
                Lexeme::CloseParen,
                Lexeme::CloseBracket,
                Lexeme::CloseBrace,
                Lexeme::Semicolon,
                Lexeme::Comma,
            ])
    }

    fn consume_remainder(&mut self, message: &str) {
        self.error(message);

        self.start(SyntaxKind::Error);

        while self.cursor < self.tokens.len() {
            self.raw_bump();
        }

        self.finish();
    }

    fn enter(&mut self) -> bool {
        if self.fatal || self.depth >= self.options.recursion_limit.unwrap_or(1000) {
            self.consume_remainder("Reached recursion limit");
            false
        } else {
            self.depth += 1;
            true
        }
    }

    fn feature(&mut self, feature: Feature, start: usize) {
        let span = range(start, self.start_offset());

        self.feature_uses.push(FeatureUse {
            feature,
            range: span,
        });

        if !self.options.features.contains(&feature) {
            self.report(span, &format!("feature {feature:?} is not enabled"));
        }
    }

    // Blocks and statement dispatch
    fn parse_block(&mut self, stops: &[Lexeme]) {
        self.start(SyntaxKind::Block);
        self.blocks += 1;

        let mut terminal = false;

        if self.enter() {
            while !self.eof() && !self.at_any(stops) {
                if self.fatal {
                    self.consume_remainder("parse error limit reached");
                    break;
                }

                let before = self.cursor;

                if terminal {
                    self.error("statement after return, break or continue");
                }

                terminal |= self.parse_statement() == StatementFlow::Terminal;

                if before == self.cursor {
                    self.recover_token();
                }
            }

            self.depth -= 1;
        }

        self.trivia();
        self.blocks -= 1;
        self.finish();
    }

    fn contextual(&self) -> bool {
        !self.nth(1).is_some_and(|token| {
            token.kind == SyntaxKind::String
                || matches!(
                    token.lexeme,
                    Lexeme::Assign
                        | Lexeme::Comma
                        | Lexeme::AddAssign
                        | Lexeme::SubtractAssign
                        | Lexeme::MultiplyAssign
                        | Lexeme::DivideAssign
                        | Lexeme::FloorDivideAssign
                        | Lexeme::ModuloAssign
                        | Lexeme::PowerAssign
                        | Lexeme::ConcatAssign
                        | Lexeme::OpenParen
                        | Lexeme::OpenBrace
                        | Lexeme::Dot
                        | Lexeme::OpenBracket
                        | Lexeme::Colon
                        | Lexeme::Less
                )
        })
    }

    fn at_attributes(&self) -> bool {
        self.at_any(&[Lexeme::Attribute, Lexeme::AttributeOpen])
    }

    fn parse_statement(&mut self) -> StatementFlow {
        self.error_kind = SyntaxKind::ErrorStatement;

        let checkpoint = self.checkpoint();
        let attributes = self.at_attributes();

        if attributes {
            self.parse_attributes();
        }

        if attributes
            && !(self.at(Lexeme::Function)
                || (self.at_any(&[
                    Lexeme::Local,
                    Lexeme::Const,
                    Lexeme::Declare,
                    Lexeme::Export,
                ]) && self.nth_at(1, Lexeme::Function)))
        {
            self.error("attributes require a function declaration");
        }

        match self.lexeme() {
            Some(Lexeme::Export) if self.contextual() => self.parse_export(checkpoint, attributes),
            Some(Lexeme::Local) => self.parse_local(checkpoint, attributes, false),
            Some(Lexeme::Const) if self.contextual() => {
                self.parse_local(checkpoint, attributes, false);
            }
            Some(Lexeme::Function) => self.parse_function_statement(checkpoint),
            Some(Lexeme::Type) if self.contextual() => self.parse_type_alias(),
            Some(Lexeme::Class | Lexeme::Open) if self.contextual() => self.parse_class(),
            Some(Lexeme::Declare) if self.contextual() => self.parse_declaration(checkpoint),
            Some(Lexeme::If) => self.parse_if_statement(),
            Some(Lexeme::While) => self.parse_while(),
            Some(Lexeme::Repeat) => self.parse_repeat(),
            Some(Lexeme::For) => self.parse_for(),
            Some(Lexeme::Do) => self.parse_do(),
            Some(Lexeme::Return) => {
                self.parse_return();
                return StatementFlow::Terminal;
            }
            Some(Lexeme::Break) => {
                self.parse_loop_control(SyntaxKind::BreakStatement);
                return StatementFlow::Terminal;
            }
            Some(Lexeme::Continue) if self.contextual() => {
                self.parse_loop_control(SyntaxKind::ContinueStatement);
                return StatementFlow::Terminal;
            }
            _ => self.parse_assignment_or_call(),
        }

        StatementFlow::Continue
    }

    // Declarations and functions
    fn parse_local(&mut self, checkpoint: Checkpoint, attributes: bool, exported_function: bool) {
        self.wrap(checkpoint, SyntaxKind::LocalStatement);

        let start = self.start_offset();
        let constant = self.at(Lexeme::Const) || exported_function;

        if !exported_function {
            self.bump();
        }

        if self.eat(Lexeme::Function) {
            self.start(SyntaxKind::FunctionStatement);
            self.parse_name();
            self.parse_function_body();
            self.finish();
        } else {
            if attributes {
                self.error("attributes require a function declaration");
            }

            let bindings = self.parse_bindings();
            let (values, multiple) = if self.eat(Lexeme::Assign) {
                self.parse_expression_list()
            } else {
                (0, false)
            };

            if constant && values != bindings && !multiple {
                self.error_range(
                    range(start, self.previous_end()),
                    "Missing initializer in const declaration",
                );
            }
        }

        self.finish_statement();
    }

    fn parse_export(&mut self, checkpoint: Checkpoint, attributes: bool) {
        self.wrap(checkpoint, SyntaxKind::ExportStatement);

        let start = self.start_offset();
        self.bump();

        if !self.at_any(&[Lexeme::Type, Lexeme::Class, Lexeme::Open]) {
            self.feature(Feature::ValueExports, start);
        }

        if !self.at(Lexeme::Type) && self.blocks != 1 {
            self.error_range(
                range(start, start + 6),
                "'export' may only be applied to top-level statements",
            );
        }

        let local = self.checkpoint();

        match self.lexeme() {
            Some(Lexeme::Function) => self.parse_local(local, attributes, true),
            Some(Lexeme::Local | Lexeme::Const) if !self.nth_at(1, Lexeme::Function) => {
                self.parse_local(local, attributes, false);
            }
            Some(Lexeme::Class | Lexeme::Open) => self.parse_class(),
            Some(Lexeme::Type) => self.parse_type_alias(),
            _ => {
                self.error("expected local, const, function, class or type after export");

                if !self.boundary() {
                    self.bump();
                }
            }
        }
        self.finish();
    }

    fn parse_function_statement(&mut self, checkpoint: Checkpoint) {
        self.wrap(checkpoint, SyntaxKind::FunctionStatement);
        self.bump();

        self.start(SyntaxKind::FunctionName);
        self.parse_name();

        while self.eat(Lexeme::Dot) {
            self.parse_name();
        }

        if self.eat(Lexeme::Colon) {
            self.parse_name();
        }

        self.finish();

        self.parse_function_body();
        self.finish_statement();
    }

    fn parse_function_body(&mut self) {
        self.start(SyntaxKind::FunctionBody);
        let vararg = self.parse_signature(false);

        let saved = (self.vararg, self.loops);
        self.vararg = vararg;
        self.loops = 0;

        self.parse_block(&[Lexeme::End]);
        self.expect(Lexeme::End);

        (self.vararg, self.loops) = saved;
        self.finish();
    }

    fn parse_signature(&mut self, declaration: bool) -> bool {
        if self.at(Lexeme::Less) {
            self.parse_generics(false);
        }

        self.start(SyntaxKind::Parameters);
        self.expect(Lexeme::OpenParen);
        let mut vararg = false;

        while !self.eof() && !self.at(Lexeme::CloseParen) {
            let before = self.cursor;

            if self.eat(Lexeme::Ellipsis) {
                vararg = true;

                if self.eat(Lexeme::Colon) {
                    self.start(SyntaxKind::TypeAnnotation);

                    if self.nth_at(1, Lexeme::Ellipsis) {
                        self.parse_type_value(TypeContext::Pack);
                    } else {
                        self.parse_type();
                    }

                    self.finish();
                } else if declaration {
                    self.error("declaration vararg must be annotated");
                }

                if self.at(Lexeme::Comma) {
                    self.error("vararg must be last");
                }

                break;
            }

            let unannotated_self = self.spelling() == "self";

            self.start(SyntaxKind::Binding);
            self.parse_name();

            if self.at(Lexeme::Colon) {
                self.parse_annotation();
            } else if declaration && !unannotated_self {
                self.error("declaration parameter must be annotated");
            }

            self.finish();

            if before == self.cursor || !self.eat(Lexeme::Comma) {
                break;
            }
            if self.at(Lexeme::CloseParen) {
                self.error("expected parameter after comma");
                break;
            }
        }

        self.expect(Lexeme::CloseParen);
        self.finish();

        if self.at_any(&[Lexeme::Colon, Lexeme::Arrow]) {
            self.start(SyntaxKind::TypeAnnotation);

            if self.at(Lexeme::Arrow) {
                self.error("function return annotations use ':'");
            }

            self.bump();
            self.parse_type_value(TypeContext::Pack);
            self.finish();
        }

        vararg
    }

    fn parse_name(&mut self) {
        self.trivia();
        self.start(SyntaxKind::Name);

        if self.kind() == Some(SyntaxKind::Identifier) {
            self.bump();
        } else {
            self.error("expected identifier");
        }

        self.finish();
    }

    fn parse_binding(&mut self) {
        self.trivia();
        self.start(SyntaxKind::Binding);
        self.parse_name();

        if self.at(Lexeme::Colon) {
            self.parse_annotation();
        }

        self.finish();
    }

    fn parse_bindings(&mut self) -> usize {
        self.parse_binding();
        let mut count = 1;

        while self.eat(Lexeme::Comma) {
            self.parse_binding();
            count += 1;
        }

        count
    }

    fn parse_annotation(&mut self) {
        self.start(SyntaxKind::TypeAnnotation);
        self.expect(Lexeme::Colon);
        self.parse_type();
        self.finish();
    }

    // Control flow
    fn parse_if_statement(&mut self) {
        self.start(SyntaxKind::IfStatement);
        let mut branches = 0;

        loop {
            branches += 1;

            if branches >= self.options.recursion_limit.unwrap_or(1000) {
                self.error("Reached recursion limit");
                break;
            }

            self.start(SyntaxKind::IfBranch);
            self.bump();
            self.parse_condition();
            self.expect(Lexeme::Then);
            self.parse_block(&[Lexeme::ElseIf, Lexeme::Else, Lexeme::End]);
            self.finish();

            if !self.at(Lexeme::ElseIf) {
                break;
            }
        }

        if self.at(Lexeme::Else) {
            self.start(SyntaxKind::IfBranch);
            self.bump();
            self.parse_block(&[Lexeme::End]);
            self.finish();
        }

        self.expect(Lexeme::End);
        self.finish_statement();
    }

    fn parse_condition(&mut self) {
        if self.at(Lexeme::Local) || (self.at(Lexeme::Const) && !self.nth_at(1, Lexeme::OpenParen))
        {
            self.start(SyntaxKind::ConditionalBinding);
            let start = self.start_offset();
            self.bump();
            self.feature(Feature::ConditionalBindings, start);

            self.parse_binding();
            self.expect(Lexeme::Assign);
            self.parse_expression(0);
            self.finish();
        } else {
            self.parse_expression(0);
        }
    }

    fn parse_while(&mut self) {
        self.start(SyntaxKind::WhileStatement);
        self.bump();
        self.parse_expression(0);
        self.expect(Lexeme::Do);
        self.parse_loop_block(Lexeme::End);
        self.finish_statement();
    }

    fn parse_repeat(&mut self) {
        self.start(SyntaxKind::RepeatStatement);
        self.bump();
        self.parse_loop_block(Lexeme::Until);
        self.parse_expression(0);
        self.finish_statement();
    }

    fn parse_for(&mut self) {
        self.start(SyntaxKind::ForStatement);
        self.bump();

        let bindings = self.parse_bindings();
        let numeric = self.eat(Lexeme::Assign);

        if !numeric {
            self.expect(Lexeme::In);
        }

        let (values, _) = self.parse_expression_list();

        if numeric && (bindings != 1 || !(2..=3).contains(&values)) {
            self.error("numeric for requires one binding and two or three expressions");
        }

        self.expect(Lexeme::Do);
        self.parse_loop_block(Lexeme::End);
        self.finish_statement();
    }

    fn parse_loop_block(&mut self, end: Lexeme) {
        self.loops += 1;
        self.parse_block(&[end]);
        self.loops -= 1;
        self.expect(end);
    }

    fn parse_do(&mut self) {
        self.start(SyntaxKind::DoStatement);
        self.bump();
        self.parse_block(&[Lexeme::End]);
        self.expect(Lexeme::End);
        self.finish_statement();
    }

    fn parse_return(&mut self) {
        self.start(SyntaxKind::ReturnStatement);
        self.bump();

        if !self.eof()
            && !self.at_any(&[
                Lexeme::End,
                Lexeme::Else,
                Lexeme::ElseIf,
                Lexeme::Until,
                Lexeme::Semicolon,
            ])
        {
            self.parse_expression_list();
        }

        self.finish_statement();
    }

    fn parse_loop_control(&mut self, kind: SyntaxKind) {
        self.start(kind);

        if self.loops == 0 {
            let message = if kind == SyntaxKind::BreakStatement {
                "break must be inside a loop in this function"
            } else {
                "continue must be inside a loop in this function"
            };

            self.error(message);
        }

        self.bump();
        self.finish_statement();
    }

    // Assignment and call statements
    fn parse_assignment_or_call(&mut self) {
        let checkpoint = self.checkpoint();
        let start = self.start_offset();
        let errors = self.errors.len();

        self.start(SyntaxKind::ExpressionList);
        let first = self.parse_expression(0);
        let mut count = 1;
        let mut assignable = first.is_variable();

        while self.eat(Lexeme::Comma) {
            assignable &= self.parse_expression(0).is_variable();
            count += 1;
        }

        self.finish();

        if self.at_any(&[
            Lexeme::Assign,
            Lexeme::AddAssign,
            Lexeme::SubtractAssign,
            Lexeme::MultiplyAssign,
            Lexeme::DivideAssign,
            Lexeme::FloorDivideAssign,
            Lexeme::ModuloAssign,
            Lexeme::PowerAssign,
            Lexeme::ConcatAssign,
        ]) {
            self.wrap(checkpoint, SyntaxKind::AssignmentStatement);
            let compound = !self.at(Lexeme::Assign);

            if !assignable {
                self.error("assignment target must be a variable or field");
            }

            self.bump();
            let (values, _) = self.parse_expression_list();

            if compound && (count != 1 || values != 1) {
                self.error("compound assignment requires one target and one expression");
            }
        } else {
            self.wrap(checkpoint, SyntaxKind::ExpressionStatement);

            if (count != 1 || !matches!(first, Expression::Call)) && self.errors.len() == errors {
                let end = self.previous_end();
                let span = range(start.min(end), start.max(end));

                self.error_range(
                    span,
                    "Incomplete statement: expected assignment or a function call",
                );
            }
        }

        self.finish_statement();
    }

    // Classes and ambient declarations
    fn parse_class(&mut self) {
        self.start(SyntaxKind::ClassStatement);
        let start = self.start_offset();
        self.eat(Lexeme::Open);
        self.expect(Lexeme::Class);
        self.feature(Feature::Classes, start);

        if self.blocks != 1 {
            self.error(&format!(
                "Cannot declare class '{}' inside another statement or expression",
                self.spelling()
            ));
        }

        self.parse_name();

        if self.eat(Lexeme::Extends) {
            self.parse_class_base();
        }

        self.start(SyntaxKind::ClassBody);

        while !self.eof() && !self.at(Lexeme::End) {
            let before = self.cursor;
            self.parse_class_member();

            if before == self.cursor {
                self.recover_token();
            }
        }

        self.finish();
        self.expect(Lexeme::End);
        self.finish_statement();
    }

    fn parse_class_base(&mut self) {
        self.start(SyntaxKind::ClassBase);
        self.start(SyntaxKind::NameExpression);
        self.parse_name();
        self.finish();

        if self.eat(Lexeme::Dot) {
            self.parse_name();
        } else if self.eat(Lexeme::OpenBracket) {
            self.parse_expression(0);
            self.expect(Lexeme::CloseBracket);
        }

        self.finish();
    }

    fn parse_class_member(&mut self) {
        let checkpoint = self.checkpoint();
        let public = self.eat(Lexeme::Public);

        if self.eat(Lexeme::Function) {
            self.wrap(checkpoint, SyntaxKind::ClassMethod);
            self.parse_name();
            self.parse_function_body();
            self.finish();
        } else if public {
            self.wrap(checkpoint, SyntaxKind::ClassProperty);
            self.parse_name();

            if self.at(Lexeme::Colon) {
                self.parse_annotation();
            }

            self.finish();
        } else {
            self.error("Only class properties and functions can be declared within a class");
            self.recover_token();
        }
    }

    fn parse_declaration(&mut self, checkpoint: Checkpoint) {
        let start = self.start_offset();
        self.bump();
        self.feature(Feature::Declarations, start);

        let previous = self.declaration_context;
        self.declaration_context = true;

        if self.eat(Lexeme::Function) {
            self.wrap(checkpoint, SyntaxKind::DeclareFunction);
            self.parse_name();
            self.parse_signature(true);
            self.finish_statement();
        } else if self.eat(Lexeme::Extern) {
            self.wrap(checkpoint, SyntaxKind::DeclareExtern);
            self.parse_extern();
            self.finish_statement();
        } else {
            self.wrap(checkpoint, SyntaxKind::DeclareGlobal);
            self.parse_name();
            self.expect(Lexeme::Colon);
            self.parse_type();
            self.finish_statement();
        }

        self.declaration_context = previous;
    }

    fn parse_extern(&mut self) {
        self.expect(Lexeme::Type);
        self.parse_name();

        if self.eat(Lexeme::Extends) {
            self.start(SyntaxKind::TypeName);
            self.parse_name();
            self.finish();
        }

        self.expect(Lexeme::With);

        self.start(SyntaxKind::ExternBody);
        let mut indexer = false;

        while !self.eof() && !self.at(Lexeme::End) {
            let before = self.cursor;
            self.parse_extern_member(&mut indexer);

            if before == self.cursor {
                self.recover_token();
            }
        }

        self.finish();
        self.expect(Lexeme::End);
    }

    fn parse_extern_member(&mut self, indexer: &mut bool) {
        let checkpoint = self.checkpoint();
        let attributes = self.at_attributes();

        if attributes {
            self.parse_attributes();
        }

        if self.eat(Lexeme::Function) {
            self.wrap(checkpoint, SyntaxKind::ExternMethod);
            self.parse_name();

            if self.at(Lexeme::Less) {
                self.error("extern methods do not support generic parameters");
            }

            self.parse_signature(true);
        } else {
            self.wrap(checkpoint, SyntaxKind::ExternProperty);

            if attributes {
                self.error("extern attributes require a method");
            }

            if self.eat(Lexeme::OpenBracket) {
                self.start(SyntaxKind::TypeIndexer);

                if *indexer {
                    self.error("only one extern indexer is allowed");
                }

                *indexer = true;
                self.parse_type();
                self.expect(Lexeme::CloseBracket);
                self.expect(Lexeme::Colon);
                self.parse_type();
                self.finish();
            } else {
                if self.at_any(&[Lexeme::Read, Lexeme::Write])
                    && self.nth_at(1, Lexeme::OpenBracket)
                {
                    self.error("extern indexers do not accept access modifiers");
                }

                self.parse_type_field(indexer, false);
            }
        }

        self.finish();
    }

    // Attributes: expand combined lexer tokens into the established Rowan shape.
    fn parse_attributes(&mut self) {
        self.start(SyntaxKind::Attributes);
        let mut names = BTreeSet::new();

        while self.at_attributes() {
            self.trivia();
            let token = self.current().expect("attribute token");
            self.emit(SyntaxKind::Symbol, token.start, token.start + 1);

            if token.lexeme == Lexeme::AttributeOpen {
                self.emit(SyntaxKind::Symbol, token.start + 1, token.end);
                self.openers.push((Lexeme::CloseBracket, token.start + 1));
                self.cursor += 1;

                if self.at(Lexeme::CloseBracket) {
                    self.error("attribute list cannot be empty");
                }

                while !self.eof() && !self.at(Lexeme::CloseBracket) {
                    self.parse_list_attribute(&mut names);

                    if !self.eat(Lexeme::Comma) {
                        break;
                    }
                    if self.at(Lexeme::CloseBracket) {
                        self.error("expected attribute after comma");
                    }
                }

                self.expect(Lexeme::CloseBracket);
            } else {
                self.start(SyntaxKind::Attribute);
                self.start(SyntaxKind::Name);

                let name = self.text[token.start + 1..token.end].to_owned();

                if name.is_empty() {
                    self.error("attribute name must immediately follow @");
                } else {
                    self.emit(SyntaxKind::Identifier, token.start + 1, token.end);
                }

                self.cursor += 1;
                self.finish();

                self.validate_attribute(&name, token.start + 1, &mut names);
                self.finish();
            }
        }

        self.finish();
    }

    fn parse_list_attribute(&mut self, names: &mut BTreeSet<String>) {
        self.trivia();
        self.start(SyntaxKind::Attribute);
        let start = self.start_offset();
        let name = self.spelling().to_owned();

        self.parse_name();
        self.validate_attribute(&name, start, names);

        if self.at_any(&[Lexeme::OpenParen, Lexeme::OpenBrace])
            || self.kind() == Some(SyntaxKind::String)
        {
            self.start(SyntaxKind::AttributeArguments);
            self.parse_arguments();
            self.finish();
        }

        self.finish();
    }

    fn validate_attribute(&mut self, name: &str, start: usize, names: &mut BTreeSet<String>) {
        match name {
            "debugnoinline" => self.feature(Feature::DebugNoInline, start),
            "native" | "checked" | "deprecated" => {}
            _ => self.error("unknown attribute"),
        }

        if !names.insert(name.into()) {
            self.error("duplicate attribute");
        }
    }

    // Type declarations and generic parameters
    fn parse_type_alias(&mut self) {
        let checkpoint = self.checkpoint();
        self.expect(Lexeme::Type);

        if self.eat(Lexeme::Function) {
            self.wrap(checkpoint, SyntaxKind::TypeFunction);
            self.parse_name();
            self.parse_function_body();
        } else {
            self.wrap(checkpoint, SyntaxKind::TypeAlias);
            self.parse_name();

            if self.at(Lexeme::Less) {
                self.parse_generics(true);
            }

            self.expect(Lexeme::Assign);
            self.parse_type();
        }

        self.finish_statement();
    }

    fn parse_generics(&mut self, defaults: bool) {
        self.start(SyntaxKind::GenericParameters);
        self.expect(Lexeme::Less);
        let mut pack_seen = false;
        let mut default_seen = false;

        loop {
            let checkpoint = self.checkpoint();
            self.parse_name();
            let pack = self.eat(Lexeme::Ellipsis);

            self.wrap(
                checkpoint,
                if pack {
                    SyntaxKind::GenericPackParameter
                } else {
                    SyntaxKind::GenericParameter
                },
            );

            if pack_seen && !pack {
                self.error("Generic types come before generic type packs");
            }

            pack_seen |= pack;

            if self.eat(Lexeme::Assign) {
                if !defaults {
                    self.error("generic defaults are only allowed in type aliases");
                }

                default_seen = true;
                self.start(SyntaxKind::TypeDefault);
                let context = if pack {
                    TypeContext::Pack
                } else {
                    TypeContext::Type
                };

                if self.parse_type_value(context) != TypeForm::Pack && pack {
                    self.error("expected type pack default");
                }

                self.finish();
            } else if default_seen {
                self.error("expected default after preceding defaulted parameter");
            }

            self.finish();

            if !self.eat(Lexeme::Comma) {
                break;
            }
            if self.at(Lexeme::Greater) {
                self.error("expected generic parameter after comma");
                break;
            }
        }

        self.expect(Lexeme::Greater);
        self.finish();
    }

    // Types and packs
    fn parse_type(&mut self) {
        self.parse_type_value(TypeContext::Type);
    }

    fn parse_type_value(&mut self, context: TypeContext) -> TypeForm {
        let previous = self.error_kind;
        self.error_kind = SyntaxKind::ErrorType;

        self.trivia();
        self.start(SyntaxKind::Type);
        let checkpoint = self.checkpoint();

        if !self.enter() {
            self.finish();
            self.error_kind = previous;
            return TypeForm::Type;
        }

        let mut form = TypeForm::Type;
        let mut union = false;
        let mut intersection = false;
        let mut length = 0;

        if !self.at_any(&[Lexeme::Pipe, Lexeme::Ampersand]) {
            form = self.parse_type_atom(context);
            length += 1;
        }

        while self.at_any(&[Lexeme::Pipe, Lexeme::Ampersand])
            || (form == TypeForm::Type && self.at(Lexeme::Question))
        {
            if self.at(Lexeme::Question) {
                union = true;
                self.start(SyntaxKind::OptionalType);
                self.bump();
                self.finish();
            } else {
                union |= self.at(Lexeme::Pipe);
                intersection |= self.at(Lexeme::Ampersand);
                self.bump();
                self.parse_type_atom(TypeContext::Type);
                length += 1;
            }

            if length > self.options.type_length_limit.unwrap_or(1000) {
                self.consume_remainder("type length limit exceeded");
                break;
            }
        }

        if union || intersection {
            self.wrap(
                checkpoint,
                if union {
                    SyntaxKind::UnionType
                } else {
                    SyntaxKind::IntersectionType
                },
            );

            if union && intersection {
                self.error("mixing union and intersection requires parentheses");
            }

            self.finish();
        }

        self.depth -= 1;
        self.finish();
        self.error_kind = previous;

        form
    }

    fn parse_type_atom(&mut self, context: TypeContext) -> TypeForm {
        self.trivia();

        match self.lexeme() {
            Some(Lexeme::Ellipsis) => self.parse_variadic_type(context),
            Some(Lexeme::Attribute | Lexeme::AttributeOpen | Lexeme::Less | Lexeme::OpenParen) => {
                self.parse_parenthesized_type(context)
            }
            Some(Lexeme::OpenBrace) => {
                self.parse_table_type();
                TypeForm::Type
            }
            Some(Lexeme::Nil | Lexeme::True | Lexeme::False) => {
                self.bump();
                TypeForm::Type
            }
            _ if self.kind() == Some(SyntaxKind::String) => {
                if self.at(Lexeme::InterpolationSimple) {
                    self.error("backtick strings cannot be used as types");
                }

                self.parse_literal();
                TypeForm::Type
            }
            Some(Lexeme::InterpolationStart) => {
                self.parse_interpolation();
                self.error("interpolated strings cannot be used as types");
                TypeForm::Type
            }
            _ if self.kind() == Some(SyntaxKind::Identifier) => self.parse_named_type(context),
            _ => {
                self.error("expected type");

                if !self.boundary()
                    && !self.at_any(&[Lexeme::Greater, Lexeme::Assign, Lexeme::Arrow])
                {
                    self.bump();
                }
                TypeForm::Type
            }
        }
    }

    fn parse_variadic_type(&mut self, context: TypeContext) -> TypeForm {
        self.start(SyntaxKind::VariadicTypePack);
        self.bump();
        self.parse_type();

        if context == TypeContext::Type {
            self.error("type pack is not allowed in this context");
        }

        self.finish();

        TypeForm::Pack
    }

    fn parse_named_type(&mut self, context: TypeContext) -> TypeForm {
        if self.nth_at(1, Lexeme::Ellipsis) {
            self.start(SyntaxKind::GenericTypePack);
            self.parse_name();
            self.bump();

            if context == TypeContext::Type {
                self.error("type pack is not allowed in this context");
            }

            self.finish();

            return TypeForm::Pack;
        }

        if self.at(Lexeme::Typeof) && !self.nth_at(1, Lexeme::Dot) {
            self.start(SyntaxKind::TypeofType);
            self.bump();
            self.expect(Lexeme::OpenParen);
            self.parse_expression(0);
            self.expect(Lexeme::CloseParen);
            self.finish();
        } else {
            self.start(SyntaxKind::TypeName);
            self.bump();

            if self.eat(Lexeme::Dot) {
                self.parse_name();
            }

            self.finish();

            if self.at(Lexeme::Less) {
                self.parse_type_arguments();
            }
        }

        TypeForm::Type
    }

    fn function_type_parentheses(&self) -> bool {
        if !self.at(Lexeme::OpenParen) {
            return false;
        }

        let mut tokens = self.tokens[self.cursor..]
            .iter()
            .filter(|token| !token.kind.is_trivia());
        tokens.next();

        let mut depth = 1;

        while let Some(token) = tokens.next() {
            match token.lexeme {
                Lexeme::OpenParen => depth += 1,
                Lexeme::CloseParen => {
                    depth -= 1;

                    if depth == 0 {
                        return tokens.next().is_some_and(|token| {
                            matches!(token.lexeme, Lexeme::Arrow | Lexeme::Colon)
                        });
                    }
                }
                _ => {}
            }
        }

        false
    }

    fn parse_parenthesized_type(&mut self, context: TypeContext) -> TypeForm {
        let checkpoint = self.checkpoint();
        let attributed = self.at_attributes();

        if attributed {
            self.parse_attributes();

            if !self.declaration_context {
                self.error("type attributes require declaration context");
            }
        }

        let generics = self.at(Lexeme::Less);

        if generics {
            self.parse_generics(false);
        }

        let function = self.function_type_parentheses();

        self.expect(Lexeme::OpenParen);
        let (count, tail, named) = self.parse_type_list(context, function);
        self.expect(Lexeme::CloseParen);

        if self.at_any(&[Lexeme::Arrow, Lexeme::Colon]) || generics || named || attributed {
            self.wrap(checkpoint, SyntaxKind::FunctionType);

            if self.eat(Lexeme::Colon) {
                self.error("function types use '->' for returns");
            } else {
                self.expect(Lexeme::Arrow);
            }

            if self.enter() {
                self.parse_type_atom(TypeContext::Pack);
                self.depth -= 1;
            }

            self.finish();

            TypeForm::Type
        } else if context == TypeContext::Pack
            && !(count == 1
                && !tail
                && self.at_any(&[Lexeme::Pipe, Lexeme::Ampersand, Lexeme::Question]))
        {
            self.wrap(checkpoint, SyntaxKind::TypePack);
            self.finish();
            TypeForm::Pack
        } else {
            self.wrap(checkpoint, SyntaxKind::TypeGroup);

            if count != 1 || tail {
                self.error("type groups require one type; packs require a pack context");
            }

            self.finish();
            TypeForm::Type
        }
    }

    fn parse_type_list(&mut self, context: TypeContext, function: bool) -> (usize, bool, bool) {
        let mut count = 0;
        let mut tail = false;
        let mut named = false;

        while !self.eof() && !self.at(Lexeme::CloseParen) {
            let before = self.cursor;

            if self.kind() == Some(SyntaxKind::Identifier) && self.nth_at(1, Lexeme::Colon) {
                self.parse_name();
                self.bump();
                named = true;
            }

            if tail {
                self.error("type pack must be last");
            }

            let allow_pack = (context == TypeContext::Pack || function)
                && !self.at_any(&[
                    Lexeme::Attribute,
                    Lexeme::AttributeOpen,
                    Lexeme::Less,
                    Lexeme::OpenParen,
                ]);

            let item_context = if allow_pack {
                TypeContext::Pack
            } else {
                TypeContext::Type
            };

            tail = self.parse_type_value(item_context) == TypeForm::Pack;
            count += 1;

            if before == self.cursor || !self.eat(Lexeme::Comma) {
                break;
            }
            if self.at(Lexeme::CloseParen) {
                self.error("expected type after comma");
                break;
            }
        }

        (count, tail, named)
    }

    fn parse_type_arguments(&mut self) {
        self.start(SyntaxKind::TypeArguments);
        self.expect(Lexeme::Less);

        if !self.at(Lexeme::Greater) {
            loop {
                let before = self.cursor;
                self.parse_type_value(TypeContext::Pack);

                if before == self.cursor || !self.eat(Lexeme::Comma) {
                    break;
                }
                if self.at(Lexeme::Greater) {
                    self.error("expected type argument after comma");
                    break;
                }
            }
        }

        self.expect(Lexeme::Greater);
        self.finish();
    }

    fn parse_table_type(&mut self) {
        self.start(SyntaxKind::TableType);
        self.bump();

        let mut indexer = false;
        let mut first = true;

        while !self.eof() && !self.at(Lexeme::CloseBrace) {
            let before = self.cursor;
            let array = self.parse_type_field(&mut indexer, first);
            first = false;

            if array {
                if !self.at(Lexeme::CloseBrace) {
                    self.error("array shorthand must be the only table type field");
                }
                break;
            }
            if before == self.cursor || !(self.eat(Lexeme::Comma) || self.eat(Lexeme::Semicolon)) {
                break;
            }
        }

        self.expect(Lexeme::CloseBrace);
        self.finish();
    }

    fn parse_type_field(&mut self, indexer: &mut bool, first: bool) -> bool {
        let checkpoint = self.checkpoint();

        if self.at_any(&[Lexeme::Read, Lexeme::Write]) && !self.nth_at(1, Lexeme::Colon) {
            self.bump();
        }

        if self.eat(Lexeme::OpenBracket) {
            let property =
                self.kind() == Some(SyntaxKind::String) && self.nth_at(1, Lexeme::CloseBracket);

            self.wrap(
                checkpoint,
                if property {
                    SyntaxKind::TypeProperty
                } else {
                    SyntaxKind::TypeIndexer
                },
            );

            if property {
                if let Some(token) = self.current()
                    && decode_string(&self.bytes[token.start..token.end])
                        .is_ok_and(|value| value.contains(&0))
                {
                    self.error("property name cannot contain NUL");
                }

                self.parse_literal();
            } else {
                if *indexer {
                    self.error("only one table indexer is allowed");
                }

                *indexer = true;
                self.parse_type();
            }

            self.expect(Lexeme::CloseBracket);
            self.expect(Lexeme::Colon);
            self.parse_type();
            self.finish();
            false
        } else if self.kind() == Some(SyntaxKind::Identifier) && self.nth_at(1, Lexeme::Colon) {
            self.wrap(checkpoint, SyntaxKind::TypeProperty);
            self.parse_name();
            self.bump();
            self.parse_type();
            self.finish();
            false
        } else if first {
            self.wrap(checkpoint, SyntaxKind::ArrayType);
            self.parse_type();
            self.finish();
            true
        } else {
            self.wrap(checkpoint, SyntaxKind::TypeProperty);
            self.parse_name();
            self.expect(Lexeme::Colon);
            self.parse_type();
            self.finish();
            false
        }
    }

    // Expressions: precedence, unary, primary, suffixes
    fn parse_expression_list(&mut self) -> (usize, bool) {
        self.trivia();
        self.start(SyntaxKind::ExpressionList);

        let mut last = self.parse_expression(0);
        let mut count = 1;

        while self.eat(Lexeme::Comma) {
            last = self.parse_expression(0);
            count += 1;
        }

        self.finish();

        (count, last.is_multiple())
    }

    fn parse_expression(&mut self, precedence: u8) -> Expression {
        let depth = self.depth;

        if !self.enter() {
            return Expression::Value;
        }

        let previous = self.error_kind;
        self.error_kind = SyntaxKind::ErrorExpression;

        let checkpoint = self.checkpoint();
        let mut result = self.parse_unary_or_primary(checkpoint);

        while let Some((left, right, confusable)) = self.binary_precedence() {
            if left <= precedence {
                break;
            }

            self.wrap(checkpoint, SyntaxKind::BinaryExpression);

            if confusable {
                self.error("use Luau operators 'and', 'or' and '~='");
                self.bump();
            }

            self.bump();
            self.parse_expression(right);
            self.finish();
            result = Expression::Value;

            if !self.enter() {
                break;
            }
        }

        self.depth = depth;
        self.error_kind = previous;

        result
    }

    fn binary_precedence(&self) -> Option<(u8, u8, bool)> {
        let confusable = matches!(
            (self.lexeme(), self.nth(1).map(|token| token.lexeme)),
            (Some(Lexeme::Ampersand), Some(Lexeme::Ampersand))
                | (Some(Lexeme::Pipe), Some(Lexeme::Pipe))
                | (Some(Lexeme::Bang), Some(Lexeme::Assign))
        );

        let (left, right) = match self.lexeme()? {
            Lexeme::Or => (1, 1),
            Lexeme::And => (2, 2),
            Lexeme::Equal
            | Lexeme::NotEqual
            | Lexeme::Less
            | Lexeme::LessEqual
            | Lexeme::Greater
            | Lexeme::GreaterEqual => (3, 3),
            Lexeme::Concat => (5, 4),
            Lexeme::Plus | Lexeme::Minus => (6, 6),
            Lexeme::Star | Lexeme::Slash | Lexeme::FloorDivide | Lexeme::Percent => (7, 7),
            Lexeme::Caret => (10, 9),
            Lexeme::Pipe if confusable => (1, 1),
            Lexeme::Ampersand if confusable => (2, 2),
            Lexeme::Bang if confusable => (3, 3),
            _ => return None,
        };

        Some((left, right, confusable))
    }

    fn parse_unary_or_primary(&mut self, checkpoint: Checkpoint) -> Expression {
        if self.at_any(&[Lexeme::Not, Lexeme::Minus, Lexeme::Hash, Lexeme::Bang]) {
            self.start(SyntaxKind::UnaryExpression);

            if self.at(Lexeme::Bang) {
                self.error("Unexpected '!'; did you mean 'not'?");
            }

            self.bump();
            self.parse_expression(8);
            self.finish();

            return Expression::Value;
        }

        let prefix = self.kind() == Some(SyntaxKind::Identifier) || self.at(Lexeme::OpenParen);
        let mut result = self.parse_primary();

        if prefix {
            result = self.parse_suffixes(checkpoint, result);
        }

        if self.eat(Lexeme::DoubleColon) {
            self.wrap(checkpoint, SyntaxKind::TypeAssertionExpression);
            self.parse_type();
            self.finish();
            result = Expression::Value;
        }

        result
    }

    fn parse_primary(&mut self) -> Expression {
        match self.lexeme() {
            Some(Lexeme::Attribute | Lexeme::AttributeOpen | Lexeme::Function) => {
                self.parse_function_expression();
            }
            Some(Lexeme::If) => self.parse_if_expression(),
            Some(Lexeme::OpenParen) => self.parse_parenthesized_expression(),
            Some(Lexeme::OpenBrace) => self.parse_table(),
            Some(Lexeme::Ellipsis) => {
                self.start(SyntaxKind::VarargExpression);

                if !self.vararg {
                    self.error("Cannot use '...' outside of a vararg function");
                }

                self.bump();
                self.finish();

                return Expression::Vararg;
            }
            Some(Lexeme::Nil | Lexeme::True | Lexeme::False) => {
                self.start(SyntaxKind::LiteralExpression);
                self.bump();
                self.finish();
            }
            _ if self.kind() == Some(SyntaxKind::Identifier) => {
                self.start(SyntaxKind::NameExpression);
                self.bump();
                self.finish();

                return Expression::Variable;
            }
            _ if matches!(self.kind(), Some(SyntaxKind::Number | SyntaxKind::String)) => {
                self.start(SyntaxKind::LiteralExpression);
                self.parse_literal();
                self.finish();
            }
            Some(Lexeme::InterpolationStart) => self.parse_interpolation(),
            _ => {
                self.error("expected expression");

                if !self.boundary() {
                    self.bump();
                }
            }
        }

        Expression::Value
    }

    fn parse_function_expression(&mut self) {
        self.start(SyntaxKind::FunctionExpression);

        if self.at_attributes() {
            self.parse_attributes();
        }

        self.expect(Lexeme::Function);
        self.parse_function_body();
        self.finish();
    }

    fn parse_if_expression(&mut self) {
        self.start(SyntaxKind::IfExpression);
        self.bump();
        self.parse_expression(0);
        self.expect(Lexeme::Then);
        self.parse_expression(0);

        let mut branches = 1;

        while self.eat(Lexeme::ElseIf) {
            branches += 1;

            if branches >= self.options.recursion_limit.unwrap_or(1000) {
                self.error("Reached recursion limit");
                break;
            }

            self.parse_expression(0);
            self.expect(Lexeme::Then);
            self.parse_expression(0);
        }

        self.expect(Lexeme::Else);
        self.parse_expression(0);
        self.finish();
    }

    fn parse_parenthesized_expression(&mut self) {
        self.start(SyntaxKind::ParenthesizedExpression);
        self.bump();
        self.parse_expression(0);
        self.expect(Lexeme::CloseParen);
        self.finish();
    }

    fn parse_suffixes(&mut self, checkpoint: Checkpoint, mut result: Expression) -> Expression {
        loop {
            match self.lexeme() {
                Some(Lexeme::Dot) => {
                    self.parse_field_suffix(checkpoint);
                    result = Expression::Variable;
                }
                Some(Lexeme::OpenBracket) => {
                    self.parse_index_suffix(checkpoint);
                    result = Expression::Variable;
                }
                Some(Lexeme::Less) if self.nth_at(1, Lexeme::Less) => {
                    self.parse_instantiation(checkpoint);
                    result = Expression::Value;
                }
                Some(Lexeme::Colon | Lexeme::OpenParen | Lexeme::OpenBrace) => {
                    self.parse_call_suffix(checkpoint);
                    result = Expression::Call;
                }
                _ if self.kind() == Some(SyntaxKind::String) => {
                    self.parse_call_suffix(checkpoint);
                    result = Expression::Call;
                }
                _ => break,
            }

            if !self.enter() {
                break;
            }
        }

        result
    }

    fn parse_field_suffix(&mut self, checkpoint: Checkpoint) {
        self.wrap(checkpoint, SyntaxKind::FieldExpression);
        self.bump();
        self.parse_name();
        self.finish();
    }

    fn parse_index_suffix(&mut self, checkpoint: Checkpoint) {
        self.wrap(checkpoint, SyntaxKind::IndexExpression);
        self.bump();
        self.parse_expression(0);
        self.expect(Lexeme::CloseBracket);
        self.finish();
    }

    fn parse_instantiation(&mut self, checkpoint: Checkpoint) {
        self.wrap(checkpoint, SyntaxKind::InstantiationExpression);
        self.bump();
        self.parse_type_arguments();
        self.expect(Lexeme::Greater);
        self.finish();
    }

    fn parse_call_suffix(&mut self, checkpoint: Checkpoint) {
        if self.eat(Lexeme::Colon) {
            self.wrap(checkpoint, SyntaxKind::MethodExpression);
            self.parse_name();
            self.finish();

            self.wrap(checkpoint, SyntaxKind::CallExpression);

            if self.at(Lexeme::Less) && self.nth_at(1, Lexeme::Less) {
                self.bump();
                self.parse_type_arguments();
                self.expect(Lexeme::Greater);
            }
        } else {
            self.wrap(checkpoint, SyntaxKind::CallExpression);
        }

        if self.at(Lexeme::OpenParen)
            && self.bytes[self.previous_end()..self.start_offset()].contains(&b'\n')
        {
            self.error("Ambiguous syntax: this looks like an argument list for a function call, but could also be a start of new statement; use ';' to separate statements");
        }

        self.parse_arguments();
        self.finish();
    }

    fn parse_arguments(&mut self) {
        self.start(SyntaxKind::Arguments);

        if self.eat(Lexeme::OpenParen) {
            if !self.at(Lexeme::CloseParen) {
                self.parse_expression_list();
            }
            self.expect(Lexeme::CloseParen);
        } else if self.at(Lexeme::OpenBrace) {
            self.parse_table();
        } else if self.kind() == Some(SyntaxKind::String) {
            if self.at(Lexeme::InterpolationSimple) {
                self.error("backtick call arguments require parentheses");
            }

            self.parse_primary();
        } else if self.at(Lexeme::InterpolationStart) {
            self.error("interpolated call arguments require parentheses");
            self.parse_interpolation();
        } else {
            self.error("expected call arguments");
        }

        self.finish();
    }

    // Tables and interpolation
    fn parse_table(&mut self) {
        self.start(SyntaxKind::TableExpression);
        self.expect(Lexeme::OpenBrace);

        while !self.eof() && !self.at(Lexeme::CloseBrace) {
            let before = self.cursor;
            self.parse_table_field();

            if before == self.cursor || !(self.eat(Lexeme::Comma) || self.eat(Lexeme::Semicolon)) {
                break;
            }
        }

        self.expect(Lexeme::CloseBrace);
        self.finish();
    }

    fn parse_table_field(&mut self) {
        self.start(SyntaxKind::TableField);

        if self.eat(Lexeme::OpenBracket) {
            self.parse_expression(0);
            self.expect(Lexeme::CloseBracket);
            self.expect(Lexeme::Assign);
            self.parse_expression(0);
        } else if self.kind() == Some(SyntaxKind::Identifier) && self.nth_at(1, Lexeme::Assign) {
            self.parse_name();
            self.bump();
            self.parse_expression(0);
        } else {
            self.parse_expression(0);
        }

        self.finish();
    }

    fn parse_interpolation(&mut self) {
        self.start(SyntaxKind::InterpolationExpression);
        self.bump();

        while !self.eof() && !self.at(Lexeme::InterpolationEnd) {
            if self.at(Lexeme::InterpolationText) {
                let token = self.current().expect("interpolation text");

                if quoted_bytes(&self.bytes[token.start..token.end]).is_err() {
                    self.error("malformed interpolation escape");
                }

                self.bump();
            } else if self.at_any(&[Lexeme::InterpolationOpen, Lexeme::InterpolationDoubleBrace]) {
                self.start(SyntaxKind::InterpolationPart);
                self.bump();
                self.parse_expression(0);

                if self.eat(Lexeme::InterpolationClose) {
                    self.finish();
                } else {
                    self.error("expected interpolation close");
                    self.finish();
                    break;
                }
            } else {
                break;
            }
        }

        if !self.eat(Lexeme::InterpolationEnd) {
            self.error("expected interpolation end");
        }

        self.finish();
    }

    fn parse_literal(&mut self) {
        if let Some(token) = self.current() {
            let bytes = &self.bytes[token.start..token.end];

            if token.kind == SyntaxKind::String && decode_string(bytes).is_err() {
                self.error("String literal contains malformed escape sequence");
            }

            if token.kind == SyntaxKind::Number {
                if number_value(&self.text[token.start..token.end]).is_none() {
                    self.error("Malformed number");
                }

                if bytes.last() == Some(&b'i') {
                    self.feature(Feature::IntegerLiterals, token.start);
                }
            }
        }

        self.bump();
    }
}

// Source coordinates and token spelling
fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(
        TextSize::try_from(start).expect("source size"),
        TextSize::try_from(end).expect("source size"),
    )
}

fn lexeme_spelling(lexeme: Lexeme) -> &'static str {
    match lexeme {
        Lexeme::OpenParen => "(",
        Lexeme::CloseParen => ")",
        Lexeme::OpenBracket => "[",
        Lexeme::CloseBracket => "]",
        Lexeme::OpenBrace => "{",
        Lexeme::CloseBrace => "}",
        Lexeme::Less => "<",
        Lexeme::Greater => ">",
        Lexeme::Assign => "=",
        Lexeme::Colon => ":",
        Lexeme::Arrow => "->",
        Lexeme::Then => "then",
        Lexeme::Else => "else",
        Lexeme::End => "end",
        Lexeme::Do => "do",
        Lexeme::Until => "until",
        Lexeme::In => "in",
        Lexeme::Class => "class",
        Lexeme::Type => "type",
        Lexeme::With => "with",
        Lexeme::Function => "function",
        _ => unreachable!("expect requires a grammar delimiter or keyword"),
    }
}

fn collect_hot_comments(bytes: &[u8], tokens: &[Token], mut header: bool) -> Vec<HotComment> {
    let mut comments = Vec::new();

    for token in tokens {
        if token.kind == SyntaxKind::Comment {
            if let Some(text) = bytes[token.start..token.end].strip_prefix(b"--!") {
                let end = text
                    .iter()
                    .rposition(|byte| !is_space(*byte))
                    .map_or(0, |end| end + 1);

                comments.push(HotComment {
                    range: range(token.start, token.end),
                    header,
                    text: text[..end].to_vec(),
                });
            }
        } else if !token.kind.is_trivia() {
            header = false;
        }
    }

    comments
}

// Literal values are decoded from original bytes, never Rowan stand-ins.
#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("malformed Luau string literal")]
pub struct StringLiteralError;

/// # Errors
/// Rejects non-string tokens and malformed escapes.
pub fn string_bytes(parse: &Parse, token: &SyntaxToken) -> Result<Vec<u8>, StringLiteralError> {
    if !matches!(
        token.kind(),
        SyntaxKind::String | SyntaxKind::InterpolationText
    ) {
        return Err(StringLiteralError);
    }

    let bytes = parse
        .source
        .slice(parse.source_range(token.text_range()))
        .map_err(|_| StringLiteralError)?;

    if token.kind() == SyntaxKind::InterpolationText {
        quoted_bytes(bytes)
    } else {
        decode_string(bytes)
    }
}

fn decode_string(bytes: &[u8]) -> Result<Vec<u8>, StringLiteralError> {
    if bytes.contains(&0) {
        return Err(StringLiteralError);
    }

    if bytes.first() == Some(&b'[') {
        let depth = bytes
            .iter()
            .skip(1)
            .take_while(|byte| **byte == b'=')
            .count();
        let delimiter = depth + 2;

        if bytes.get(delimiter - 1) != Some(&b'[') || bytes.len() < delimiter * 2 {
            return Err(StringLiteralError);
        }

        let close = bytes.len() - delimiter;

        if bytes[close] != b']'
            || bytes.last() != Some(&b']')
            || !bytes[close + 1..bytes.len() - 1]
                .iter()
                .all(|byte| *byte == b'=')
        {
            return Err(StringLiteralError);
        }

        let body = &bytes[delimiter..close];
        let body = body
            .strip_prefix(b"\r\n")
            .or_else(|| body.strip_prefix(b"\n"))
            .unwrap_or(body);

        let mut output = Vec::new();
        let mut input = body.iter().copied().peekable();

        while let Some(byte) = input.next() {
            if byte != b'\r' || input.peek() != Some(&b'\n') {
                output.push(byte);
            }
        }

        return Ok(output);
    }

    if !matches!(bytes.first(), Some(b'\'' | b'"' | b'`')) || bytes.first() != bytes.last() {
        return Err(StringLiteralError);
    }

    quoted_bytes(
        bytes
            .get(1..bytes.len().checked_sub(1).ok_or(StringLiteralError)?)
            .ok_or(StringLiteralError)?,
    )
}

fn is_space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 11
}

fn quoted_bytes(body: &[u8]) -> Result<Vec<u8>, StringLiteralError> {
    let mut input = body.iter().copied().peekable();
    let mut output = Vec::new();

    while let Some(byte) = input.next() {
        if byte != b'\\' {
            output.push(byte);
            continue;
        }

        match input.next().ok_or(StringLiteralError)? {
            0 => return Err(StringLiteralError),
            b'a' => output.push(7),
            b'b' => output.push(8),
            b'f' => output.push(12),
            b'n' | b'\n' => output.push(b'\n'),
            b'r' => output.push(b'\r'),
            b't' => output.push(b'\t'),
            b'v' => output.push(11),
            b'\r' => {
                output.push(b'\n');

                if input.peek() == Some(&b'\n') {
                    input.next();
                }
            }
            b'z' => {
                while input.peek().is_some_and(|byte| is_space(*byte)) {
                    input.next();
                }
            }
            b'x' => {
                let mut code = 0;

                for _ in 0..2 {
                    code = code * 16
                        + char::from(input.next().ok_or(StringLiteralError)?)
                            .to_digit(16)
                            .ok_or(StringLiteralError)?;
                }

                output.push(u8::try_from(code).map_err(|_| StringLiteralError)?);
            }
            b'u' => decode_unicode_escape(&mut input, &mut output)?,
            first @ b'0'..=b'9' => {
                let mut code = u16::from(first - b'0');

                for _ in 0..2 {
                    if let Some(next) = input.next_if(u8::is_ascii_digit) {
                        code = code * 10 + u16::from(next - b'0');
                    } else {
                        break;
                    }
                }

                output.push(u8::try_from(code).map_err(|_| StringLiteralError)?);
            }
            other => output.push(other),
        }
    }

    Ok(output)
}

fn decode_unicode_escape(
    input: &mut std::iter::Peekable<impl Iterator<Item = u8>>,
    output: &mut Vec<u8>,
) -> Result<(), StringLiteralError> {
    if input.next() != Some(b'{') {
        return Err(StringLiteralError);
    }

    let mut code = 0_u32;
    let mut digits = 0;

    while input.peek() != Some(&b'}') {
        let digit = char::from(input.next().ok_or(StringLiteralError)?)
            .to_digit(16)
            .ok_or(StringLiteralError)?;

        code = code.wrapping_mul(16).wrapping_add(digit);
        digits += 1;

        if digits > 16 {
            return Err(StringLiteralError);
        }
    }

    input.next();

    if digits == 0 {
        return Err(StringLiteralError);
    }

    if (0xd800..=0xdfff).contains(&code) {
        for byte in [
            0xe0 | (code >> 12),
            0x80 | ((code >> 6) & 0x3f),
            0x80 | (code & 0x3f),
        ] {
            output.push(u8::try_from(byte).map_err(|_| StringLiteralError)?);
        }
    } else {
        let scalar = char::from_u32(code).ok_or(StringLiteralError)?;
        output.extend_from_slice(scalar.encode_utf8(&mut [0; 4]).as_bytes());
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum NumberValue {
    Float(f64),
    Integer(i64),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberStatus {
    Exact,
    Imprecise,
    Overflow,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumberLiteral {
    pub value: NumberValue,
    pub status: NumberStatus,
}

/// # Returns
/// `None` for malformed numerals or overflowing signed integer literals.
#[must_use]
#[expect(
    clippy::cast_precision_loss,
    reason = "Luau numbers are doubles; precision loss is reported as metadata"
)]
pub fn number_value(text: &str) -> Option<NumberLiteral> {
    let spelling = text.replace('_', "");
    let integer = spelling.ends_with('i');
    let text = spelling.strip_suffix('i').unwrap_or(&spelling);

    let radix = if text.starts_with("0x") || text.starts_with("0X") {
        16
    } else if text.starts_with("0b") || text.starts_with("0B") {
        2
    } else {
        10
    };

    if radix != 10 {
        let digits = &text[2..];

        if digits.is_empty() || !digits.chars().all(|character| character.is_digit(radix)) {
            return None;
        }

        let parsed = u64::from_str_radix(digits, radix);

        if integer {
            return parsed.ok().map(|value| NumberLiteral {
                value: NumberValue::Integer(i64::from_ne_bytes(value.to_ne_bytes())),
                status: NumberStatus::Exact,
            });
        }

        let overflow = parsed.is_err();
        let value = parsed.unwrap_or(u64::MAX);
        let float = value as f64;

        let status = if overflow {
            NumberStatus::Overflow
        } else if format!("{float:.0}") != value.to_string() {
            NumberStatus::Imprecise
        } else {
            NumberStatus::Exact
        };

        Some(NumberLiteral {
            value: NumberValue::Float(float),
            status,
        })
    } else if integer {
        if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }

        text.parse().ok().map(|value| NumberLiteral {
            value: NumberValue::Integer(value),
            status: NumberStatus::Exact,
        })
    } else {
        let value = text.parse::<f64>().ok()?;

        let status = if text.bytes().all(|byte| byte.is_ascii_digit())
            && value >= 9_007_199_254_740_992.0
            && format!("{value:.0}") != text
        {
            NumberStatus::Imprecise
        } else {
            NumberStatus::Exact
        };

        Some(NumberLiteral {
            value: NumberValue::Float(value),
            status,
        })
    }
}
