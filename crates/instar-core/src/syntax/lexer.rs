use std::ops::Range;

use logos::Logos;

use super::SyntaxKind;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Token {
    pub(super) lexeme: Lexeme,
    pub(super) kind: SyntaxKind,
    pub(super) start: usize,
    pub(super) end: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct LexError {
    pub(super) range: Range<usize>,
    pub(super) message: &'static str,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Termination {
    Closed,
    Unfinished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuoteStyle {
    Single,
    Double,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct QuotedString {
    pub(super) quote: QuoteStyle,
    pub(super) termination: Termination,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct LongBracket {
    pub(super) depth: usize,
    pub(super) termination: Termination,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CommentForm {
    Line,
    Block(LongBracket),
}

#[derive(Logos, Clone, Copy, Debug, Eq, PartialEq)]
#[logos(utf8 = false)]
pub(super) enum Lexeme {
    // Trivia
    #[token("--", Lexer::read_comment)]
    Comment(CommentForm),
    #[regex(r"[ \t\r\n\x0B\x0C]+")]
    Whitespace,

    // Names and keywords
    #[token("and")]
    And,
    #[token("@", Lexer::read_attribute)]
    Attribute,
    #[token("break")]
    Break,
    #[token("checked")]
    Checked,
    #[token("class")]
    Class,
    #[token("const")]
    Const,
    #[token("continue")]
    Continue,
    #[token("debugnoinline")]
    DebugNoInline,
    #[token("declare")]
    Declare,
    #[token("deprecated")]
    Deprecated,
    #[token("do")]
    Do,
    #[token("else")]
    Else,
    #[token("elseif")]
    ElseIf,
    #[token("end")]
    End,
    #[token("export")]
    Export,
    #[token("extends")]
    Extends,
    #[token("extern")]
    Extern,
    #[token("false")]
    False,
    #[token("for")]
    For,
    #[token("function")]
    Function,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Identifier,
    #[token("if")]
    If,
    #[token("in")]
    In,
    #[token("local")]
    Local,
    #[token("native")]
    Native,
    #[token("nil")]
    Nil,
    #[token("not")]
    Not,
    #[token("open")]
    Open,
    #[token("or")]
    Or,
    #[token("public")]
    Public,
    #[token("read")]
    Read,
    #[token("repeat")]
    Repeat,
    #[token("return")]
    Return,
    #[token("then")]
    Then,
    #[token("true")]
    True,
    #[token("type")]
    Type,
    #[token("typeof")]
    Typeof,
    #[token("until")]
    Until,
    #[token("while")]
    While,
    #[token("with")]
    With,
    #[token("write")]
    Write,

    // Literals
    #[regex(r"\[=*\[", Lexer::read_long_string)]
    LongString(LongBracket),
    #[regex(r"[0-9]|\.[0-9]", Lexer::read_number)]
    Number,
    #[regex(r#"["']"#, Lexer::read_quoted_string)]
    QuotedString(QuotedString),

    // Operators
    #[token("+=")]
    AddAssign,
    #[token("&")]
    Ampersand,
    #[token("=")]
    Assign,
    #[token("!")]
    Bang,
    #[token("^")]
    Caret,
    #[token("..")]
    Concat,
    #[token("..=")]
    ConcatAssign,
    #[token("/=")]
    DivideAssign,
    #[token("==")]
    Equal,
    #[token("//")]
    FloorDivide,
    #[token("//=")]
    FloorDivideAssign,
    #[token(">")]
    Greater,
    #[token(">=")]
    GreaterEqual,
    #[token("#")]
    Hash,
    #[token("<")]
    Less,
    #[token("<=")]
    LessEqual,
    #[token("-")]
    Minus,
    #[token("%=")]
    ModuloAssign,
    #[token("*=")]
    MultiplyAssign,
    #[token("~=")]
    NotEqual,
    #[token("%")]
    Percent,
    #[token("|")]
    Pipe,
    #[token("+")]
    Plus,
    #[token("^=")]
    PowerAssign,
    #[token("?")]
    Question,
    #[token("/")]
    Slash,
    #[token("*")]
    Star,
    #[token("-=")]
    SubtractAssign,
    #[token("~")]
    Tilde,

    // Delimiters and punctuation
    #[token("->")]
    Arrow,
    #[token("@[")]
    AttributeOpen,
    #[token("}")]
    CloseBrace,
    #[token("]")]
    CloseBracket,
    #[token(")")]
    CloseParen,
    #[token(":")]
    Colon,
    #[token(",")]
    Comma,
    #[token(".")]
    Dot,
    #[token("::")]
    DoubleColon,
    #[token("...")]
    Ellipsis,
    #[token("{")]
    OpenBrace,
    #[token("[")]
    OpenBracket,
    #[token("(")]
    OpenParen,
    #[token(";")]
    Semicolon,

    // Interpolation: Logos matches the opening backtick; the reader emits the rest.
    InterpolationClose,
    InterpolationDoubleBrace,
    InterpolationEnd,
    InterpolationOpen,
    InterpolationSimple,
    #[token("`")]
    InterpolationStart,
    InterpolationText,

    // Invalid input and unrecognized characters retained for parser recovery
    #[regex(br"[\x80-\xFF]", Lexer::read_unicode)]
    BrokenUnicode(Option<u32>),
    #[regex(r"[\x01-\x7F]", |lexer| lexer.slice()[0], priority = 0)]
    Character(u8),
    InvalidByte,
    #[token("\0")]
    InvalidNul,
    #[regex(r"\[=+")]
    MalformedLongBracket,
}

impl Lexeme {
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive token classification keeps every lexeme mapping explicit"
    )]
    fn syntax_kind(self) -> SyntaxKind {
        match self {
            Self::Whitespace => SyntaxKind::Whitespace,
            Self::And
            | Self::Break
            | Self::Do
            | Self::Else
            | Self::ElseIf
            | Self::End
            | Self::False
            | Self::For
            | Self::Function
            | Self::If
            | Self::In
            | Self::Local
            | Self::Nil
            | Self::Not
            | Self::Or
            | Self::Repeat
            | Self::Return
            | Self::Then
            | Self::True
            | Self::Until
            | Self::While => SyntaxKind::Keyword,
            Self::Identifier
            | Self::Const
            | Self::Continue
            | Self::Declare
            | Self::Export
            | Self::Type
            | Self::Typeof
            | Self::Class
            | Self::Open
            | Self::Extends
            | Self::Public
            | Self::Extern
            | Self::With
            | Self::Read
            | Self::Write
            | Self::Native
            | Self::Checked
            | Self::Deprecated
            | Self::DebugNoInline => SyntaxKind::Identifier,
            Self::Number => SyntaxKind::Number,
            Self::Comment(_) => SyntaxKind::Comment,
            Self::LongString(LongBracket {
                termination: Termination::Closed,
                ..
            })
            | Self::QuotedString(QuotedString {
                termination: Termination::Closed,
                ..
            })
            | Self::InterpolationSimple => SyntaxKind::String,
            Self::LongString(LongBracket {
                termination: Termination::Unfinished,
                ..
            })
            | Self::QuotedString(QuotedString {
                termination: Termination::Unfinished,
                ..
            })
            | Self::MalformedLongBracket
            | Self::BrokenUnicode(_)
            | Self::InvalidNul
            | Self::InvalidByte => SyntaxKind::Invalid,
            Self::InterpolationStart => SyntaxKind::InterpolationStart,
            Self::InterpolationText => SyntaxKind::InterpolationText,
            Self::InterpolationOpen | Self::InterpolationDoubleBrace => {
                SyntaxKind::InterpolationOpen
            }
            Self::InterpolationClose => SyntaxKind::InterpolationClose,
            Self::InterpolationEnd => SyntaxKind::InterpolationEnd,
            Self::Attribute
            | Self::AttributeOpen
            | Self::Equal
            | Self::NotEqual
            | Self::LessEqual
            | Self::GreaterEqual
            | Self::Concat
            | Self::Ellipsis
            | Self::Arrow
            | Self::DoubleColon
            | Self::FloorDivide
            | Self::AddAssign
            | Self::SubtractAssign
            | Self::MultiplyAssign
            | Self::DivideAssign
            | Self::FloorDivideAssign
            | Self::ModuloAssign
            | Self::PowerAssign
            | Self::ConcatAssign
            | Self::Plus
            | Self::Minus
            | Self::Star
            | Self::Slash
            | Self::Percent
            | Self::Caret
            | Self::Hash
            | Self::Assign
            | Self::Less
            | Self::Greater
            | Self::Tilde
            | Self::Bang
            | Self::OpenParen
            | Self::CloseParen
            | Self::OpenBracket
            | Self::CloseBracket
            | Self::OpenBrace
            | Self::CloseBrace
            | Self::Semicolon
            | Self::Colon
            | Self::Comma
            | Self::Dot
            | Self::Question
            | Self::Pipe
            | Self::Ampersand
            | Self::Character(_) => SyntaxKind::Symbol,
        }
    }

    fn error_message(self) -> Option<&'static str> {
        match self {
            Self::Comment(CommentForm::Block(LongBracket {
                termination: Termination::Unfinished,
                ..
            })) => Some("unfinished comment"),
            Self::LongString(LongBracket {
                termination: Termination::Unfinished,
                ..
            })
            | Self::QuotedString(QuotedString {
                termination: Termination::Unfinished,
                ..
            }) => Some("unterminated string"),
            Self::MalformedLongBracket => Some("malformed long string delimiter"),
            Self::BrokenUnicode(Some(_)) => {
                Some("Unicode character is not allowed outside strings and comments")
            }
            Self::BrokenUnicode(None) => Some("invalid UTF-8 sequence"),
            Self::InvalidNul => Some("NUL byte is not allowed outside strings and comments"),
            Self::InvalidByte => Some("invalid byte"),
            Self::InterpolationDoubleBrace => {
                Some("double braces are not permitted in interpolation; use '\\{'")
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
enum InterpolationMode {
    Text,
    Expression { braces: usize },
}

struct Lexer<'source> {
    inner: logos::Lexer<'source, Lexeme>,
    modes: Vec<InterpolationMode>,
    errors: Vec<LexError>,
}

pub(super) fn lex(bytes: &[u8]) -> (Vec<Token>, Vec<LexError>) {
    let mut lexer = Lexer {
        inner: Lexeme::lexer(bytes),
        modes: Vec::new(),
        errors: Vec::new(),
    };
    let mut tokens = Vec::new();
    while let Some(token) = lexer.next_token() {
        tokens.push(token);
    }
    if !lexer.modes.is_empty() {
        lexer.unfinished_interpolation(bytes.len());
    }
    (tokens, lexer.errors)
}

impl Lexer<'_> {
    // Token dispatch
    fn next_token(&mut self) -> Option<Token> {
        while !self.inner.remainder().is_empty() {
            let start = self.offset();
            let lexeme = if matches!(self.modes.last(), Some(InterpolationMode::Text)) {
                if matches!(self.inner.remainder().first(), Some(b'\n' | b'\r')) {
                    self.modes.pop();
                    self.unfinished_interpolation(start);
                    continue;
                }
                self.read_interpolation_text()
            } else {
                self.read_expression_token()?
            };
            return Some(self.finish_token(start, lexeme));
        }
        None
    }

    fn read_expression_token(&mut self) -> Option<Lexeme> {
        let lexeme = self.inner.next()?.unwrap_or(Lexeme::InvalidByte);
        Some(match lexeme {
            Lexeme::InterpolationStart => self.open_interpolation(),
            Lexeme::OpenBrace => self.open_brace(),
            Lexeme::CloseBrace => self.close_brace(),
            other => other,
        })
    }

    // Token finalization
    fn finish_token(&mut self, start: usize, lexeme: Lexeme) -> Token {
        let range = start..self.offset();
        if let Some(message) = lexeme.error_message() {
            self.errors.push(LexError {
                range: range.clone(),
                message,
            });
        }
        Token {
            lexeme,
            kind: lexeme.syntax_kind(),
            start: range.start,
            end: range.end,
        }
    }

    fn offset(&self) -> usize {
        self.inner.source().len() - self.inner.remainder().len()
    }

    // Names
    fn read_attribute(lexer: &mut logos::Lexer<'_, Lexeme>) {
        let bytes = lexer.remainder();
        if !bytes
            .first()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || *byte == b'_')
        {
            return;
        }
        let end = bytes
            .iter()
            .take_while(|byte| byte.is_ascii_alphanumeric() || **byte == b'_')
            .count();
        lexer.bump(end);
    }

    // Numbers
    fn read_number(lexer: &mut logos::Lexer<'_, Lexeme>) {
        let bytes = lexer.remainder();
        let mut end = 0;
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_digit() || matches!(byte, b'.' | b'_'))
        {
            end += 1;
        }
        if matches!(bytes.get(end), Some(b'e' | b'E')) {
            end += 1;
            if matches!(bytes.get(end), Some(b'+' | b'-')) {
                end += 1;
            }
        }
        while bytes
            .get(end)
            .is_some_and(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
        {
            end += 1;
        }
        lexer.bump(end);
    }

    // Comments and strings
    fn read_comment(lexer: &mut logos::Lexer<'_, Lexeme>) -> CommentForm {
        if let Some(opening) = long_bracket_open(lexer.remainder()) {
            lexer.bump(opening);
            CommentForm::Block(Self::read_long_body(lexer, opening - 2))
        } else {
            let end = lexer
                .remainder()
                .iter()
                .position(|byte| matches!(byte, b'\n' | b'\r'))
                .unwrap_or(lexer.remainder().len());
            lexer.bump(end);
            CommentForm::Line
        }
    }

    fn read_long_string(lexer: &mut logos::Lexer<'_, Lexeme>) -> LongBracket {
        let depth = lexer.slice().len() - 2;
        Self::read_long_body(lexer, depth)
    }

    fn read_quoted_string(lexer: &mut logos::Lexer<'_, Lexeme>) -> QuotedString {
        let delimiter = lexer.slice()[0];
        let quote = if delimiter == b'\'' {
            QuoteStyle::Single
        } else {
            QuoteStyle::Double
        };
        let bytes = lexer.remainder();
        let mut end = 0;
        while let Some(&byte) = bytes.get(end) {
            match byte {
                byte if byte == delimiter => {
                    lexer.bump(end + 1);
                    return QuotedString {
                        quote,
                        termination: Termination::Closed,
                    };
                }
                b'\n' | b'\r' => break,
                b'\\' => end = escape_end(bytes, end),
                _ => end += 1,
            }
        }
        lexer.bump(end);
        QuotedString {
            quote,
            termination: Termination::Unfinished,
        }
    }

    fn read_long_body(lexer: &mut logos::Lexer<'_, Lexeme>, depth: usize) -> LongBracket {
        let termination = if let Some(end) = long_bracket_end(lexer.remainder(), depth) {
            lexer.bump(end);
            Termination::Closed
        } else {
            lexer.bump(lexer.remainder().len());
            Termination::Unfinished
        };
        LongBracket { depth, termination }
    }

    // Interpolation: opening, text, nested braces, recovery
    fn open_interpolation(&mut self) -> Lexeme {
        let end = interpolation_text_end(self.inner.remainder());
        if self.inner.remainder().get(end) == Some(&b'`') {
            self.inner.bump(end + 1);
            Lexeme::InterpolationSimple
        } else {
            self.modes.push(InterpolationMode::Text);
            Lexeme::InterpolationStart
        }
    }

    fn read_interpolation_text(&mut self) -> Lexeme {
        let bytes = self.inner.remainder();
        let (lexeme, width) = match bytes[0] {
            b'`' => {
                self.modes.pop();
                (Lexeme::InterpolationEnd, 1)
            }
            b'{' => {
                *self.modes.last_mut().expect("interpolation text mode") =
                    InterpolationMode::Expression { braces: 0 };
                if bytes.get(1) == Some(&b'{') {
                    (Lexeme::InterpolationDoubleBrace, 2)
                } else {
                    (Lexeme::InterpolationOpen, 1)
                }
            }
            _ => (Lexeme::InterpolationText, interpolation_text_end(bytes)),
        };
        self.inner.bump(width);
        lexeme
    }

    fn open_brace(&mut self) -> Lexeme {
        if let Some(InterpolationMode::Expression { braces }) = self.modes.last_mut() {
            *braces += 1;
        }
        Lexeme::OpenBrace
    }

    fn close_brace(&mut self) -> Lexeme {
        match self.modes.last_mut() {
            Some(mode @ InterpolationMode::Expression { braces: 0 }) => {
                *mode = InterpolationMode::Text;
                Lexeme::InterpolationClose
            }
            Some(InterpolationMode::Expression { braces }) => {
                *braces -= 1;
                Lexeme::CloseBrace
            }
            _ => Lexeme::CloseBrace,
        }
    }

    fn unfinished_interpolation(&mut self, offset: usize) {
        self.errors.push(LexError {
            range: offset..offset,
            message: "unterminated interpolation",
        });
    }

    // Invalid Unicode
    fn read_unicode(lexer: &mut logos::Lexer<'_, Lexeme>) -> Option<u32> {
        let first = lexer.slice()[0];
        let (width, mut codepoint) = match first {
            0xC0..=0xDF => (2, u32::from(first & 0x1F)),
            0xE0..=0xEF => (3, u32::from(first & 0x0F)),
            0xF0..=0xF7 => (4, u32::from(first & 0x07)),
            _ => return None,
        };
        let mut consumed = 0;
        for &byte in lexer.remainder().iter().take(width - 1) {
            if byte & 0xC0 != 0x80 {
                break;
            }
            codepoint = (codepoint << 6) | u32::from(byte & 0x3F);
            consumed += 1;
        }
        lexer.bump(consumed);
        (consumed == width - 1).then_some(codepoint)
    }
}

fn long_bracket_open(bytes: &[u8]) -> Option<usize> {
    if bytes.first() != Some(&b'[') {
        return None;
    }
    let mut end = 1;
    while bytes.get(end) == Some(&b'=') {
        end += 1;
    }
    (bytes.get(end) == Some(&b'[')).then_some(end + 1)
}

fn long_bracket_end(bytes: &[u8], depth: usize) -> Option<usize> {
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b']' {
            cursor += 1;
            continue;
        }
        let mut end = cursor + 1;
        while bytes.get(end) == Some(&b'=') {
            end += 1;
        }
        if end - cursor - 1 == depth && bytes.get(end) == Some(&b']') {
            return Some(end + 1);
        }
        cursor = end;
    }
    None
}

fn escape_end(bytes: &[u8], slash: usize) -> usize {
    let mut end = slash + 1;
    match bytes.get(end) {
        Some(b'z') => {
            end += 1;
            while bytes.get(end).is_some_and(|byte| is_space(*byte)) {
                end += 1;
            }
        }
        Some(b'\r') => {
            end += 1;
            if bytes.get(end) == Some(&b'\n') {
                end += 1;
            }
        }
        Some(_) => end += 1,
        None => {}
    }
    end
}

fn is_space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == b'\x0B'
}

fn interpolation_text_end(bytes: &[u8]) -> usize {
    let mut end = 0;
    while let Some(&byte) = bytes.get(end) {
        match byte {
            b'`' | b'{' | b'\n' | b'\r' => break,
            b'\\' if bytes[end..].starts_with(b"\\u{") => end += 3,
            b'\\' => end = escape_end(bytes, end),
            _ => end += 1,
        }
    }
    end
}
