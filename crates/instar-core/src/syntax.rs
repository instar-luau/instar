//! Lossless, revision-owned Luau syntax with explicit feature availability.
//! Grammar errors retain their source text. Binding-dependent checks live in semantics.

use std::{collections::BTreeSet, sync::Arc};

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, Language};
use text_size::{TextRange, TextSize};

use crate::source::{Source, SourceError};

macro_rules! kinds {
    ($($kind:ident),+ $(,)?) => {
        /// Structural nodes and lossless lexical tokens. Keyword and symbol text
        /// distinguishes individual operators; names in bindings are not references.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(u16)]
        pub enum SyntaxKind { $($kind),+ }
        const KINDS: &[SyntaxKind] = &[$(SyntaxKind::$kind),+];
    };
}

kinds! {
    Root, Block, LocalStatement, FunctionStatement, FunctionExpression,
    FunctionBody, Parameters, Binding, FunctionName, Name, TypeAnnotation,
    TypeAlias, GenericParameters, Type, TypeName, TypeField, TypeofType,
    IfStatement, IfBranch, WhileStatement, RepeatStatement, ForStatement,
    DoStatement, ReturnStatement, BreakStatement, ContinueStatement,
    AssignmentStatement, ExpressionStatement, ExpressionList,
    NameExpression, LiteralExpression, VarargExpression, UnaryExpression,
    BinaryExpression, ParenthesizedExpression, CallExpression, Arguments,
    IndexExpression, FieldExpression, MethodExpression, TableExpression,
    TableField, IfExpression, TypeAssertionExpression, InterpolationExpression,
    InterpolationPart, ExportStatement, ConditionalBinding, ClassStatement,
    ClassBase, ClassBody, ClassProperty, ClassMethod, TypeFunction,
    DeclareGlobal, DeclareFunction, DeclareExtern, ExternBody, ExternProperty,
    ExternMethod, Attributes, Attribute, AttributeArguments,
    GenericParameter, GenericPackParameter, TypeDefault, TypeArguments,
    TypeGroup, FunctionType, TypePack, GenericTypePack, VariadicTypePack,
    UnionType, IntersectionType, OptionalType, TableType, TypeProperty,
    TypeIndexer, ArrayType, InstantiationExpression,
    Error, ErrorStatement, ErrorExpression, ErrorType,
    Whitespace, Comment, Identifier, Keyword, Number, String, Symbol,
    InterpolationStart, InterpolationText, InterpolationOpen, InterpolationClose,
    InterpolationEnd, Invalid,
}

impl SyntaxKind {
    #[must_use]
    pub const fn is_trivia(self) -> bool {
        matches!(self, Self::Whitespace | Self::Comment)
    }
}

/// Rowan language marker.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Luau {}

impl Language for Luau {
    type Kind = SyntaxKind;

    fn kind_from_raw(raw: rowan::SyntaxKind) -> Self::Kind {
        KINDS[usize::from(raw.0)]
    }

    fn kind_to_raw(kind: Self::Kind) -> rowan::SyntaxKind {
        rowan::SyntaxKind(kind as u16)
    }
}

pub type SyntaxNode = rowan::SyntaxNode<Luau>;
pub type SyntaxToken = rowan::SyntaxToken<Luau>;
pub type SyntaxElement = rowan::SyntaxElement<Luau>;

/// A syntax failure in original UTF-8 byte coordinates, including empty missing-token ranges.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseError {
    pub range: TextRange,
    pub message: String,
}

/// Upstream experimental flags and the declaration-mode parse option.
/// All are disabled by upstream's source defaults; callers select their language surface.
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

/// Syntax and errors retain the exact source snapshot, never the store's later revision.
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

impl Parse {
    /// Parse valid UTF-8; recoverable syntax failures are retained in `errors()`.
    ///
    /// # Errors
    /// Rejects byte-only, non-UTF-8 sources without replacement decoding.
    pub fn new(source: Arc<Source>) -> Result<Self, SourceError> {
        Self::with_options(source, ParseOptions::default())
    }

    /// # Errors
    /// Rejects non-UTF-8 source; grammar and feature errors remain in the result.
    pub fn with_options(source: Arc<Source>, options: ParseOptions) -> Result<Self, SourceError> {
        Self::entry(source, options, EntryPoint::Module)
    }

    /// Parse a complete module, expression or type against one source revision.
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

    /// Parse a source range without copying it into a different source revision.
    /// Syntax ranges are relative to the fragment; `extent` maps them to the source.
    /// # Errors
    /// Rejects invalid UTF-8 boundaries or ranges.
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
        let (tokens, errors) = lex(bytes, text);
        let mut header = extent.start() == TextSize::from(0);
        let mut hot_comments = Vec::new();
        for token in &tokens {
            if token.kind == SyntaxKind::Comment {
                if let Some(comment) = bytes[token.start..token.end].strip_prefix(b"--!") {
                    hot_comments.push(HotComment {
                        range: range(token.start, token.end),
                        header,
                        text: comment[..comment
                            .iter()
                            .rposition(|byte| !is_space(*byte))
                            .map_or(0, |end| end + 1)]
                            .to_vec(),
                    });
                }
            } else if !token.kind.is_trivia() {
                header = false;
            }
        }
        let mut parser = Parser {
            text,
            bytes,
            tokens,
            pos: 0,
            builder: GreenNodeBuilder::new(),
            errors,
            depth: 0,
            options: &options,
            feature_uses: Vec::new(),
            blocks: 0,
            loops: 0,
            vararg: true,
            declaration_context: false,
            error_kind: SyntaxKind::ErrorStatement,
            openers: Vec::new(),
        };
        parser.start(SyntaxKind::Root);
        match entry {
            EntryPoint::Module => parser.block(&[]),
            EntryPoint::Expression => {
                parser.expression(0);
            }
            EntryPoint::Type => {
                parser.ty();
            }
        }
        if !parser.eof() {
            parser.remainder_error("expected end of input");
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

    /// UTF-8 tree view. Malformed source bytes occupy same-length SUB placeholders.
    /// Read original contents through `source().slice(source_range(token.text_range()))`.
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

    /// Translate fragment-relative syntax coordinates to original source coordinates.
    #[must_use]
    pub fn source_range(&self, relative: TextRange) -> TextRange {
        relative + self.extent.start()
    }

    /// Nonempty means downstream facts may be incomplete. Error nodes are not scopes.
    #[must_use]
    pub fn errors(&self) -> &[ParseError] {
        &self.errors
    }
}

/// Decode a token from its owning parse using original source bytes and Luau's rules.
///
/// # Errors
/// Rejects non-string tokens and malformed escapes. Bytes need not be UTF-8.
pub fn string_bytes(parse: &Parse, token: &SyntaxToken) -> Result<Vec<u8>, StringLiteralError> {
    if !matches!(
        token.kind(),
        SyntaxKind::String | SyntaxKind::InterpolationText
    ) {
        return Err(StringLiteralError);
    }
    let text = parse
        .source
        .slice(parse.source_range(token.text_range()))
        .map_err(|_| StringLiteralError)?;
    if token.kind() == SyntaxKind::InterpolationText {
        return quoted_bytes(text);
    }
    decode_string(text)
}

fn decode_string(text: &[u8]) -> Result<Vec<u8>, StringLiteralError> {
    if text.contains(&0) {
        return Err(StringLiteralError);
    }
    if let Some((equals, start)) = long_open(text, 0) {
        if long_end(text, start, equals) != Some(text.len()) {
            return Err(StringLiteralError);
        }
        let body = text
            .get(start..text.len().checked_sub(start).ok_or(StringLiteralError)?)
            .ok_or(StringLiteralError)?;
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
    if !matches!(text.first(), Some(b'\'' | b'"' | b'`')) || text.first() != text.last() {
        return Err(StringLiteralError);
    }
    let body = text
        .get(1..text.len().checked_sub(1).ok_or(StringLiteralError)?)
        .ok_or(StringLiteralError)?;
    quoted_bytes(body)
}

#[derive(Clone, Copy, Debug, thiserror::Error)]
#[error("malformed Luau string literal")]
pub struct StringLiteralError;

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
                    let digit = char::from(input.next().ok_or(StringLiteralError)?)
                        .to_digit(16)
                        .ok_or(StringLiteralError)?;
                    code = code * 16 + digit;
                }
                output.push(u8::try_from(code).map_err(|_| StringLiteralError)?);
            }
            b'u' => {
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
                    // Lexer::fixupQuotedString accepts up to sixteen hexadecimal digits.
                    if digits > 16 {
                        return Err(StringLiteralError);
                    }
                }
                input.next();
                if digits == 0 {
                    return Err(StringLiteralError);
                }
                if (0xd800..=0xdfff).contains(&code) {
                    // Luau strings are bytes and its UTF-8 encoder permits surrogate code points.
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
            }
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

#[derive(Clone, Copy)]
struct Token {
    kind: SyntaxKind,
    start: usize,
    end: usize,
}

fn range(start: usize, end: usize) -> TextRange {
    // Source rejects lengths >= u32::MAX before parsing.
    TextRange::new(
        TextSize::try_from(start).expect("source size"),
        TextSize::try_from(end).expect("source size"),
    )
}

fn long_open(bytes: &[u8], start: usize) -> Option<(usize, usize)> {
    if bytes.get(start) != Some(&b'[') {
        return None;
    }
    let mut end = start + 1;
    while bytes.get(end) == Some(&b'=') {
        end += 1;
    }
    (bytes.get(end) == Some(&b'[')).then_some((end - start - 1, end + 1))
}

fn long_end(bytes: &[u8], mut pos: usize, equals: usize) -> Option<usize> {
    while pos < bytes.len() {
        if bytes[pos] == b']' {
            let mut end = pos + 1;
            while bytes.get(end) == Some(&b'=') {
                end += 1;
            }
            if end - pos - 1 == equals && bytes.get(end) == Some(&b']') {
                return Some(end + 1);
            }
        }
        pos += 1;
    }
    None
}

fn escape(bytes: &[u8], mut pos: usize) -> usize {
    pos += 1;
    match bytes.get(pos) {
        Some(b'z') => {
            pos += 1;
            while bytes.get(pos).is_some_and(|byte| is_space(*byte)) {
                pos += 1;
            }
        }
        Some(b'u') if bytes.get(pos + 1) == Some(&b'{') => {
            pos += 2;
            while bytes.get(pos).is_some_and(u8::is_ascii_hexdigit) {
                pos += 1;
            }
            if bytes.get(pos) == Some(&b'}') {
                pos += 1;
            }
        }
        Some(b'\r') => {
            pos += 1;
            if bytes.get(pos) == Some(&b'\n') {
                pos += 1;
            }
        }
        Some(_) => pos += 1,
        None => {}
    }
    pos
}

fn is_space(byte: u8) -> bool {
    byte.is_ascii_whitespace() || byte == 11
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

/// Decode Luau decimal/hexadecimal/binary numerals, including integer bit patterns.
/// Returns None for malformed numerals or overflowing signed integer literals.
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
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
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
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok().map(|value| NumberLiteral {
            value: NumberValue::Integer(value),
            status: NumberStatus::Exact,
        })
    } else {
        let value = text.parse::<f64>().ok()?;
        let status = if text.bytes().all(|b| b.is_ascii_digit())
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

#[derive(Clone, Copy)]
enum InterpolationMode {
    Text,
    Expression(usize),
}

// Delimiters are separate tokens, so interpolation expressions use the same parser
// and name-resolution boundary as ordinary expressions (including nested strings).
#[expect(
    clippy::too_many_lines,
    reason = "lexical dispatch keeps mode transitions together"
)]
fn lex(bytes: &[u8], text: &str) -> (Vec<Token>, Vec<ParseError>) {
    use SyntaxKind as K;
    let mut tokens = Vec::new();
    let mut errors = Vec::new();
    let mut modes = Vec::new();
    let mut pos = 0;
    while pos < bytes.len() {
        let start = pos;
        let kind;
        if matches!(modes.last(), Some(InterpolationMode::Text)) {
            match bytes[pos] {
                b'`' => {
                    pos += 1;
                    modes.pop();
                    kind = K::InterpolationEnd;
                }
                b'{' => {
                    let double = bytes.get(pos + 1) == Some(&b'{');
                    if double {
                        errors.push(ParseError {
                            range: range(pos, pos + 2),
                            message: "double braces are not permitted in interpolation; use '\\{'"
                                .into(),
                        });
                    }
                    pos += if double { 2 } else { 1 };
                    *modes.last_mut().expect("mode") = InterpolationMode::Expression(0);
                    kind = K::InterpolationOpen;
                }
                b'\n' | b'\r' => {
                    modes.pop();
                    errors.push(ParseError {
                        range: range(pos, pos),
                        message: "unterminated interpolation".into(),
                    });
                    continue;
                }
                _ => {
                    while pos < bytes.len() && !matches!(bytes[pos], b'`' | b'{' | b'\n' | b'\r') {
                        pos = if bytes[pos] == b'\\' {
                            escape(bytes, pos)
                        } else {
                            pos + 1
                        };
                    }
                    kind = K::InterpolationText;
                }
            }
        } else if is_space(bytes[pos]) {
            pos += 1;
            while bytes.get(pos).is_some_and(|byte| is_space(*byte)) {
                pos += 1;
            }
            kind = K::Whitespace;
        } else if bytes[pos..].starts_with(b"--") {
            pos += 2;
            if let Some((equals, body)) = long_open(bytes, pos) {
                if let Some(end) = long_end(bytes, body, equals) {
                    pos = end;
                } else {
                    pos = bytes.len();
                    errors.push(ParseError {
                        range: range(start, pos),
                        message: "unterminated long comment".into(),
                    });
                }
            } else {
                while pos < bytes.len() && !matches!(bytes[pos], b'\n' | b'\r') {
                    pos += 1;
                }
            }
            kind = K::Comment;
        } else if let Some((equals, body)) = long_open(bytes, pos) {
            if let Some(end) = long_end(bytes, body, equals) {
                pos = end;
                kind = K::String;
            } else {
                pos = bytes.len();
                kind = K::Invalid;
            }
        } else if matches!(bytes[pos], b'\'' | b'"') {
            let quote = bytes[pos];
            pos += 1;
            while pos < bytes.len() && bytes[pos] != quote && !matches!(bytes[pos], b'\n' | b'\r') {
                pos = if bytes[pos] == b'\\' {
                    escape(bytes, pos)
                } else {
                    pos + 1
                };
            }
            if bytes.get(pos) == Some(&quote) {
                pos += 1;
                kind = K::String;
            } else {
                kind = K::Invalid;
            }
        } else if bytes[pos] == b'`' {
            pos += 1;
            modes.push(InterpolationMode::Text);
            kind = K::InterpolationStart;
        } else if bytes[pos] == b'}'
            && matches!(modes.last(), Some(InterpolationMode::Expression(0)))
        {
            pos += 1;
            *modes.last_mut().expect("mode") = InterpolationMode::Text;
            kind = K::InterpolationClose;
        } else if bytes[pos].is_ascii_alphabetic() || bytes[pos] == b'_' {
            pos += 1;
            while bytes
                .get(pos)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                pos += 1;
            }
            kind = match &text[start..pos] {
                "and" | "break" | "do" | "else" | "elseif" | "end" | "false" | "for"
                | "function" | "if" | "in" | "local" | "nil" | "not" | "or" | "repeat"
                | "return" | "then" | "true" | "until" | "while" => K::Keyword,
                _ => K::Identifier,
            };
        } else if bytes[pos].is_ascii_digit()
            || (bytes[pos] == b'.' && bytes.get(pos + 1).is_some_and(u8::is_ascii_digit))
        {
            // Match Lexer::readNumber: consume a number-like pattern before
            // validation, including malformed runs such as 1..2.
            pos += 1;
            while bytes
                .get(pos)
                .is_some_and(|c| c.is_ascii_digit() || matches!(c, b'.' | b'_'))
            {
                pos += 1;
            }
            if matches!(bytes.get(pos), Some(b'e' | b'E')) {
                pos += 1;
                if matches!(bytes.get(pos), Some(b'+' | b'-')) {
                    pos += 1;
                }
            }
            while bytes
                .get(pos)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                pos += 1;
            }
            kind = if number_value(&text[start..pos]).is_some() {
                K::Number
            } else {
                K::Invalid
            };
        } else {
            if let Some(InterpolationMode::Expression(depth)) = modes.last_mut() {
                if bytes[pos] == b'{' {
                    *depth += 1;
                } else if bytes[pos] == b'}' {
                    *depth -= 1;
                }
            }
            let width = [
                "...", "..=", "//=", "==", "~=", "<=", ">=", "+=", "-=", "*=", "/=", "%=", "^=",
                "::", "->", "//", "..",
            ]
            .iter()
            .find(|symbol| text[pos..].starts_with(**symbol))
            .map_or(1, |symbol| symbol.len());
            kind = if b"+-*/%^#=<>~(){}[];:,.?|&@!".contains(&bytes[pos]) {
                K::Symbol
            } else {
                K::Invalid
            };
            pos += width;
        }
        // An invalid non-ASCII character or escaped UTF-8 scalar is retained whole.
        while !text.is_char_boundary(pos) {
            pos += 1;
        }
        if kind == K::Invalid {
            errors.push(ParseError {
                range: range(start, pos),
                message: "invalid token, numeral or unterminated string".into(),
            });
        }
        tokens.push(Token {
            kind,
            start,
            end: pos,
        });
    }
    if !modes.is_empty() {
        errors.push(ParseError {
            range: range(pos, pos),
            message: "unterminated interpolation".into(),
        });
    }
    // A backtick string with no substitutions is a constant string in Luau.
    let mut combined = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        if tokens[index].kind == K::InterpolationStart {
            let end = index
                + 1
                + usize::from(
                    tokens
                        .get(index + 1)
                        .is_some_and(|token| token.kind == K::InterpolationText),
                );
            if tokens
                .get(end)
                .is_some_and(|token| token.kind == K::InterpolationEnd)
            {
                combined.push(Token {
                    kind: K::String,
                    start: tokens[index].start,
                    end: tokens[end].end,
                });
                index = end + 1;
                continue;
            }
        }
        combined.push(tokens[index]);
        index += 1;
    }
    errors.truncate(100);
    (combined, errors)
}

struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    tokens: Vec<Token>,
    pos: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<ParseError>,
    depth: usize,
    options: &'a ParseOptions,
    feature_uses: Vec<FeatureUse>,
    blocks: usize,
    loops: usize,
    vararg: bool,
    declaration_context: bool,
    error_kind: SyntaxKind,
    openers: Vec<(String, usize)>,
}

#[derive(Clone, Copy, Default)]
struct Expression {
    lvalue: bool,
    call: bool,
    multiple: bool,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum TypeForm {
    Type,
    Pack,
}

impl Parser<'_> {
    fn current(&self) -> Option<Token> {
        self.tokens[self.pos..]
            .iter()
            .copied()
            .find(|token| !token.kind.is_trivia())
    }
    fn nth(&self, n: usize) -> &str {
        self.tokens[self.pos..]
            .iter()
            .filter(|token| !token.kind.is_trivia())
            .nth(n)
            .map_or("", |token| &self.text[token.start..token.end])
    }
    fn at(&self, text: &str) -> bool {
        self.nth(0) == text
    }
    fn function_type_parentheses(&self) -> bool {
        let mut tokens = self.tokens[self.pos..]
            .iter()
            .filter(|token| !token.kind.is_trivia());
        let Some(first) = tokens.next() else {
            return false;
        };
        if &self.text[first.start..first.end] != "(" {
            return false;
        }
        let mut depth = 1;
        while let Some(token) = tokens.next() {
            match &self.text[token.start..token.end] {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return tokens.next().is_some_and(|token| {
                            matches!(&self.text[token.start..token.end], "->" | ":")
                        });
                    }
                }
                _ => {}
            }
        }
        false
    }
    fn kind(&self) -> Option<SyntaxKind> {
        self.current().map(|token| token.kind)
    }
    fn eof(&self) -> bool {
        self.current().is_none()
    }
    fn start(&mut self, kind: SyntaxKind) {
        self.builder.start_node(Luau::kind_to_raw(kind));
    }
    fn finish(&mut self) {
        self.builder.finish_node();
    }
    fn raw_bump(&mut self) {
        if let Some(token) = self.tokens.get(self.pos) {
            self.builder.token(
                Luau::kind_to_raw(token.kind),
                &self.text[token.start..token.end],
            );
            self.pos += 1;
        }
    }
    fn trivia(&mut self) {
        while self
            .tokens
            .get(self.pos)
            .is_some_and(|token| token.kind.is_trivia())
        {
            self.raw_bump();
        }
    }
    fn bump(&mut self) {
        self.trivia();
        if let Some(token) = self.tokens.get(self.pos) {
            let text = &self.text[token.start..token.end];
            let closing = match text {
                "(" => Some(")"),
                "[" => Some("]"),
                "{" => Some("}"),
                _ => None,
            };
            if let Some(close) = closing {
                self.openers.push((close.into(), token.start));
            } else if self.openers.last().is_some_and(|(close, _)| close == text) {
                self.openers.pop();
            }
        }
        self.raw_bump();
    }
    fn eat(&mut self, text: &str) -> bool {
        if self.at(text) && !self.eof() {
            self.bump();
            true
        } else {
            false
        }
    }
    fn report(&mut self, range: TextRange, message: &str) {
        if self.errors.last().is_some_and(|error| error.range == range) {
            return;
        }
        // Parser.cpp's LuauParseErrorLimit source default.
        if self.errors.len() < 100 {
            self.errors.push(ParseError {
                range,
                message: message.into(),
            });
        }
    }
    fn error(&mut self, message: &str) {
        let token = self.current();
        let span = token.map_or(range(self.text.len(), self.text.len()), |token| {
            range(token.start, token.end)
        });
        self.report(span, message);
        self.start(self.error_kind);
        self.start(SyntaxKind::Error);
        self.finish();
        self.finish();
    }
    fn expect(&mut self, text: &str) {
        if self.eat(text) {
            return;
        }
        let origin = self
            .openers
            .iter()
            .rev()
            .find(|(close, _)| close == text)
            .map(|(_, start)| *start);
        let message = origin.map_or_else(
            || format!("expected {text}, got {}", self.nth(0)),
            |start| {
                format!(
                    "expected {text} to close delimiter at byte {start}, got {}",
                    self.nth(0)
                )
            },
        );
        self.error(&message);
        if self.nth(1) == text {
            self.start(self.error_kind);
            self.start(SyntaxKind::Error);
            self.bump();
            self.finish();
            self.finish();
            self.bump();
        }
    }
    fn feature(&mut self, feature: Feature, start: usize) {
        let span = range(
            start,
            self.current().map_or(self.text.len(), |token| token.start),
        );
        self.feature_uses.push(FeatureUse {
            feature,
            range: span,
        });
        if !self.options.features.contains(&feature) {
            self.report(span, &format!("feature {feature:?} is not enabled"));
        }
    }
    fn start_offset(&self) -> usize {
        self.current().map_or(self.text.len(), |token| token.start)
    }
    fn finish_statement(&mut self) {
        self.eat(";");
        self.finish();
    }
    fn boundary(&self) -> bool {
        self.eof()
            || matches!(
                self.nth(0),
                "end"
                    | "else"
                    | "elseif"
                    | "until"
                    | "then"
                    | "do"
                    | "local"
                    | "return"
                    | "for"
                    | "while"
                    | "repeat"
                    | ")"
                    | "]"
                    | "}"
                    | ";"
                    | ","
            )
    }
    fn checkpoint(&mut self) -> Checkpoint {
        self.trivia();
        self.builder.checkpoint()
    }
    fn wrap(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder
            .start_node_at(checkpoint, Luau::kind_to_raw(kind));
    }
    fn enter(&mut self) -> bool {
        // Parser.cpp's LuauRecursionLimit source default.
        if self.depth >= 1000 {
            self.remainder_error("syntax nesting limit exceeded");
            false
        } else {
            self.depth += 1;
            true
        }
    }
    fn remainder_error(&mut self, message: &str) {
        self.error(message);
        // Only a fatal resource limit or trailing fragment text consumes the remainder.
        self.start(SyntaxKind::Error);
        while self.pos < self.tokens.len() {
            self.raw_bump();
        }
        self.finish();
    }
    fn name(&mut self) {
        self.trivia();
        self.start(SyntaxKind::Name);
        if self.kind() == Some(SyntaxKind::Identifier) {
            self.bump();
        } else {
            self.error("expected identifier");
        }
        self.finish();
    }
    fn binding(&mut self) {
        self.trivia();
        self.start(SyntaxKind::Binding);
        self.name();
        if self.at(":") {
            self.annotation();
        }
        self.finish();
    }
    fn annotation(&mut self) {
        self.start(SyntaxKind::TypeAnnotation);
        self.expect(":");
        self.ty();
        self.finish();
    }
    fn block(&mut self, stops: &[&str]) {
        self.start(SyntaxKind::Block);
        self.blocks += 1;
        let mut terminal = false;
        if self.enter() {
            while !self.eof() && !stops.contains(&self.nth(0)) {
                if self.errors.len() >= 100 {
                    self.remainder_error("parse error limit reached");
                    break;
                }
                let before = self.pos;
                if terminal {
                    self.error("statement after return, break or continue");
                }
                terminal |= self.statement();
                if self.pos == before {
                    self.start(SyntaxKind::ErrorStatement);
                    self.start(SyntaxKind::Error);
                    self.bump();
                    self.finish();
                    self.finish();
                }
            }
            self.depth -= 1;
        }
        self.trivia();
        self.blocks -= 1;
        self.finish();
    }
    fn contextual(&self) -> bool {
        let next = self.tokens[self.pos..]
            .iter()
            .filter(|token| !token.kind.is_trivia())
            .nth(1);
        !next.is_some_and(|token| {
            token.kind == SyntaxKind::String
                || matches!(
                    &self.text[token.start..token.end],
                    "=" | ","
                        | "+="
                        | "-="
                        | "*="
                        | "/="
                        | "//="
                        | "%="
                        | "^="
                        | "..="
                        | "("
                        | "{"
                        | "."
                        | "["
                        | ":"
                        | "<"
                )
        })
    }

    #[expect(
        clippy::too_many_lines,
        reason = "statement dispatch preserves lexical and recovery boundaries"
    )]
    fn statement(&mut self) -> bool {
        use SyntaxKind as K;
        self.error_kind = K::ErrorStatement;
        self.trivia();
        let checkpoint = self.checkpoint();
        let attributes = if self.at("@") {
            self.attributes();
            true
        } else {
            false
        };
        let mut terminal = false;
        if attributes
            && !(self.at("function")
                || (matches!(self.nth(0), "local" | "const" | "declare" | "export")
                    && self.nth(1) == "function"))
        {
            self.error("attributes require a function declaration");
        }
        match self.nth(0) {
            "export" if self.contextual() => {
                self.wrap(checkpoint, K::ExportStatement);
                let start = self.start_offset();
                self.bump();
                if !matches!(self.nth(0), "type" | "class" | "open") {
                    self.feature(Feature::ValueExports, start);
                }
                if !self.at("type") && self.blocks != 1 {
                    self.error("export values must be top-level");
                }
                match self.nth(0) {
                    "function" => {
                        self.start(K::LocalStatement);
                        self.local_statement(attributes, true);
                        self.finish();
                    }
                    "local" | "const" if self.nth(1) != "function" => {
                        self.start(K::LocalStatement);
                        self.local_statement(attributes, false);
                        self.finish();
                    }
                    "class" | "open" => self.class_statement(),
                    "type" => self.type_alias(),
                    _ => {
                        self.error("expected local, const, function, class or type after export");
                        if !self.boundary() {
                            self.bump();
                        }
                    }
                }
                self.finish();
            }
            "local" => {
                self.wrap(checkpoint, K::LocalStatement);
                self.local_statement(attributes, false);
                self.finish();
            }
            "const" if self.contextual() => {
                self.wrap(checkpoint, K::LocalStatement);
                self.local_statement(attributes, false);
                self.finish();
            }
            "function" => {
                self.wrap(checkpoint, K::FunctionStatement);
                self.bump();
                self.start(K::FunctionName);
                self.name();
                while self.eat(".") {
                    self.name();
                }
                if self.eat(":") {
                    self.name();
                }
                self.finish();
                self.function_body();
                self.finish_statement();
            }
            "type" if self.contextual() => self.type_alias(),
            "class" | "open" if self.contextual() => self.class_statement(),
            "declare" if self.contextual() => self.declaration(),
            "if" => {
                self.start(K::IfStatement);
                loop {
                    self.start(K::IfBranch);
                    self.bump();
                    if self.at("local") || (self.at("const") && self.nth(1) != "(") {
                        self.start(K::ConditionalBinding);
                        let start = self.start_offset();
                        self.bump();
                        self.feature(Feature::ConditionalBindings, start);
                        self.binding();
                        self.expect("=");
                        self.expression(0);
                        self.finish();
                    } else {
                        self.expression(0);
                    }
                    self.expect("then");
                    self.block(&["elseif", "else", "end"]);
                    self.finish();
                    if !self.at("elseif") {
                        break;
                    }
                }
                if self.at("else") {
                    self.start(K::IfBranch);
                    self.bump();
                    self.block(&["end"]);
                    self.finish();
                }
                self.expect("end");
                self.finish_statement();
            }
            "while" => {
                self.start(K::WhileStatement);
                self.bump();
                self.expression(0);
                self.expect("do");
                self.loops += 1;
                self.block(&["end"]);
                self.loops -= 1;
                self.expect("end");
                self.finish_statement();
            }
            "repeat" => {
                self.start(K::RepeatStatement);
                self.bump();
                self.loops += 1;
                self.block(&["until"]);
                self.loops -= 1;
                self.expect("until");
                self.expression(0);
                self.finish_statement();
            }
            "for" => {
                self.start(K::ForStatement);
                self.bump();
                self.binding();
                let mut bindings = 1;
                while self.eat(",") {
                    self.binding();
                    bindings += 1;
                }
                let numeric = self.eat("=");
                if !numeric {
                    self.expect("in");
                }
                let (values, _) = self.expressions();
                if numeric && (bindings != 1 || !(2..=3).contains(&values)) {
                    self.error("numeric for requires one binding and two or three expressions");
                }
                self.expect("do");
                self.loops += 1;
                self.block(&["end"]);
                self.loops -= 1;
                self.expect("end");
                self.finish_statement();
            }
            "do" => {
                self.start(K::DoStatement);
                self.bump();
                self.block(&["end"]);
                self.expect("end");
                self.finish_statement();
            }
            "return" => {
                self.start(K::ReturnStatement);
                self.bump();
                if !self.eof() && !matches!(self.nth(0), "end" | "else" | "elseif" | "until" | ";")
                {
                    self.expressions();
                }
                self.finish_statement();
                terminal = true;
            }
            "break" => {
                self.start(K::BreakStatement);
                if self.loops == 0 {
                    self.error("break must be inside a loop in this function");
                }
                self.bump();
                self.finish_statement();
                terminal = true;
            }
            "continue" if self.contextual() => {
                self.start(K::ContinueStatement);
                if self.loops == 0 {
                    self.error("continue must be inside a loop in this function");
                }
                self.bump();
                self.finish_statement();
                terminal = true;
            }
            _ => {
                let checkpoint = self.checkpoint();
                self.start(K::ExpressionList);
                let first = self.expression(0);
                let mut count = 1;
                let mut lvalue = first.lvalue;
                while self.eat(",") {
                    lvalue &= self.expression(0).lvalue;
                    count += 1;
                }
                self.finish();
                if matches!(
                    self.nth(0),
                    "=" | "+=" | "-=" | "*=" | "/=" | "//=" | "%=" | "^=" | "..="
                ) {
                    self.wrap(checkpoint, K::AssignmentStatement);
                    let compound = !self.at("=");
                    if !lvalue {
                        self.error("assignment target must be a variable or field");
                    }
                    self.bump();
                    let (values, _) = self.expressions();
                    if compound && (count != 1 || values != 1) {
                        self.error("compound assignment requires one target and one expression");
                    }
                } else {
                    self.wrap(checkpoint, K::ExpressionStatement);
                    if count != 1 || !first.call {
                        self.error("expected assignment or function call");
                    }
                }
                self.finish_statement();
            }
        }
        terminal
    }

    fn local_statement(&mut self, attributes: bool, exported_function: bool) {
        use SyntaxKind as K;
        let constant = self.at("const") || exported_function;
        if !exported_function {
            self.bump();
        }
        if self.eat("function") {
            self.start(K::FunctionStatement);
            self.name();
            self.function_body();
            self.finish();
        } else {
            if attributes {
                self.error("attributes require a function declaration");
            }
            self.binding();
            let mut bindings = 1;
            while self.eat(",") {
                self.binding();
                bindings += 1;
            }
            let values = if self.eat("=") {
                self.expressions()
            } else {
                (0, false)
            };
            if constant && values.0 != bindings && !values.1 {
                self.error("missing initializer in const declaration");
            }
        }
        self.eat(";");
    }

    fn class_statement(&mut self) {
        use SyntaxKind as K;
        self.start(K::ClassStatement);
        let start = self.start_offset();
        self.eat("open");
        self.expect("class");
        self.feature(Feature::Classes, start);
        if self.blocks != 1 {
            self.error("class declarations must be top-level");
        }
        self.name();
        if self.eat("extends") {
            self.start(K::ClassBase);
            self.start(K::NameExpression);
            self.name();
            self.finish();
            if self.eat(".") {
                self.name();
            } else if self.eat("[") {
                self.expression(0);
                self.expect("]");
            }
            self.finish();
        }
        self.start(K::ClassBody);
        while !self.eof() && !self.at("end") {
            let before = self.pos;
            self.trivia();
            let checkpoint = self.checkpoint();
            let public = self.eat("public");
            if self.eat("function") {
                self.wrap(checkpoint, K::ClassMethod);
                self.name();
                self.function_body();
                self.finish();
            } else if public {
                self.wrap(checkpoint, K::ClassProperty);
                self.name();
                if self.at(":") {
                    self.annotation();
                }
                self.finish();
            } else {
                self.error("expected public property or method in class");
                self.bump();
            }
            if before == self.pos {
                self.bump();
            }
        }
        self.finish();
        self.expect("end");
        self.finish_statement();
    }

    fn declaration(&mut self) {
        use SyntaxKind as K;
        let checkpoint = self.checkpoint();
        let start = self.start_offset();
        self.bump();
        self.feature(Feature::Declarations, start);
        let previous = self.declaration_context;
        self.declaration_context = true;
        if self.eat("function") {
            self.wrap(checkpoint, K::DeclareFunction);
            self.name();
            self.signature(true);
            self.finish_statement();
        } else if self.eat("extern") {
            self.wrap(checkpoint, K::DeclareExtern);
            self.expect("type");
            self.name();
            if self.eat("extends") {
                self.start(K::TypeName);
                self.name();
                self.finish();
            }
            self.expect("with");
            self.start(K::ExternBody);
            let mut indexer = false;
            while !self.eof() && !self.at("end") {
                let before = self.pos;
                self.trivia();
                let checkpoint = self.checkpoint();
                let attributes = self.at("@");
                if attributes {
                    self.attributes();
                }
                if self.eat("function") {
                    self.wrap(checkpoint, K::ExternMethod);
                    self.name();
                    if self.at("<") {
                        self.error("extern methods do not support generic parameters");
                    }
                    self.signature(true);
                    self.finish();
                } else {
                    self.wrap(checkpoint, K::ExternProperty);
                    if attributes {
                        self.error("extern attributes require a method");
                    }
                    if self.eat("[") {
                        self.start(K::TypeIndexer);
                        if indexer {
                            self.error("only one extern indexer is allowed");
                        }
                        indexer = true;
                        self.ty();
                        self.expect("]");
                        self.expect(":");
                        self.ty();
                        self.finish();
                    } else {
                        if matches!(self.nth(0), "read" | "write") && self.nth(1) == "[" {
                            self.error("extern indexers do not accept access modifiers");
                        }
                        self.type_field(&mut indexer, false);
                    }
                    self.finish();
                }
                if before == self.pos {
                    self.bump();
                }
            }
            self.finish();
            self.expect("end");
            self.finish_statement();
        } else {
            self.wrap(checkpoint, K::DeclareGlobal);
            self.name();
            self.expect(":");
            self.ty();
            self.finish_statement();
        }
        self.declaration_context = previous;
    }

    fn attributes(&mut self) {
        use SyntaxKind as K;
        self.start(K::Attributes);
        let mut names = BTreeSet::new();
        while self.at("@") {
            let at = self.current().expect("attribute");
            self.bump();
            if self.start_offset() != at.end {
                self.error("attribute name must immediately follow @");
            }
            let list = self.eat("[");
            if list && self.at("]") {
                self.error("attribute list cannot be empty");
            }
            loop {
                let start = self.start_offset();
                self.start(K::Attribute);
                let name = self.nth(0).to_owned();
                self.name();
                if name == "debugnoinline" {
                    self.feature(Feature::DebugNoInline, start);
                } else if !matches!(name.as_str(), "native" | "checked" | "deprecated") {
                    self.error("unknown attribute");
                }
                if !names.insert(name) {
                    self.error("duplicate attribute");
                }
                if list && (matches!(self.nth(0), "(" | "{") || self.kind() == Some(K::String)) {
                    self.start(K::AttributeArguments);
                    self.arguments();
                    self.finish();
                }
                self.finish();
                if !list || !self.eat(",") {
                    break;
                }
            }
            if list {
                self.expect("]");
            }
        }
        self.finish();
    }
    fn function_body(&mut self) {
        self.start(SyntaxKind::FunctionBody);
        let vararg = self.signature(false);
        let saved = (self.vararg, self.loops);
        self.vararg = vararg;
        self.loops = 0;
        self.block(&["end"]);
        self.expect("end");
        (self.vararg, self.loops) = saved;
        self.finish();
    }

    fn signature(&mut self, declaration: bool) -> bool {
        use SyntaxKind as K;
        if self.at("<") {
            self.generics(false);
        }
        self.start(K::Parameters);
        self.expect("(");
        let mut vararg = false;
        while !self.eof() && !self.at(")") {
            let before = self.pos;
            if self.eat("...") {
                vararg = true;
                if self.eat(":") {
                    self.start(K::TypeAnnotation);
                    if self.nth(1) == "..." {
                        self.type_value(true);
                    } else {
                        self.ty();
                    }
                    self.finish();
                } else if declaration {
                    self.error("declaration vararg must be annotated");
                }
                if self.at(",") {
                    self.error("vararg must be last");
                }
                break;
            }
            let name = self.nth(0).to_owned();
            self.start(K::Binding);
            self.name();
            if self.at(":") {
                self.annotation();
            } else if declaration && name != "self" {
                self.error("declaration parameter must be annotated");
            }
            self.finish();
            if self.pos == before || !self.eat(",") {
                break;
            }
            if self.at(")") {
                self.error("expected parameter after comma");
                break;
            }
        }
        self.expect(")");
        self.finish();
        if self.at(":") || self.at("->") {
            self.start(K::TypeAnnotation);
            if self.at("->") {
                self.error("function return annotations use ':'");
            }
            self.bump();
            self.type_value(true);
            self.finish();
        }
        vararg
    }

    fn generics(&mut self, defaults: bool) {
        use SyntaxKind as K;
        self.start(K::GenericParameters);
        self.expect("<");
        let mut pack_seen = false;
        let mut default_seen = false;
        loop {
            let checkpoint = self.checkpoint();
            self.name();
            let pack = self.eat("...");
            self.wrap(
                checkpoint,
                if pack {
                    K::GenericPackParameter
                } else {
                    K::GenericParameter
                },
            );
            if pack_seen && !pack {
                self.error("generic types must precede generic packs");
            }
            pack_seen |= pack;
            if self.eat("=") {
                if !defaults {
                    self.error("generic defaults are only allowed in type aliases");
                }
                default_seen = true;
                self.start(K::TypeDefault);
                let form = self.type_value(pack);
                if pack && form != TypeForm::Pack {
                    self.error("expected type pack default");
                }
                self.finish();
            } else if default_seen {
                self.error("expected default after preceding defaulted parameter");
            }
            self.finish();
            if !self.eat(",") {
                break;
            }
            if self.at(">") {
                self.error("expected generic parameter after comma");
                break;
            }
        }
        self.expect(">");
        self.finish();
    }

    fn type_alias(&mut self) {
        use SyntaxKind as K;
        let checkpoint = self.checkpoint();
        self.expect("type");
        if self.eat("function") {
            self.wrap(checkpoint, K::TypeFunction);
            self.name();
            self.function_body();
        } else {
            self.wrap(checkpoint, K::TypeAlias);
            self.name();
            if self.at("<") {
                self.generics(true);
            }
            self.expect("=");
            self.ty();
        }
        self.finish_statement();
    }

    fn ty(&mut self) {
        self.type_value(false);
    }

    fn type_value(&mut self, allow_pack: bool) -> TypeForm {
        use SyntaxKind as K;
        let previous = self.error_kind;
        self.error_kind = K::ErrorType;
        self.trivia();
        self.start(K::Type);
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
        if !matches!(self.nth(0), "|" | "&") {
            form = self.type_atom(allow_pack);
            length += 1;
        }
        while matches!(self.nth(0), "|" | "&") || form == TypeForm::Type && self.at("?") {
            if self.at("?") {
                union = true;
                self.start(K::OptionalType);
                self.bump();
                self.finish();
            } else {
                union |= self.at("|");
                intersection |= self.at("&");
                self.bump();
                self.type_atom(false);
                length += 1;
            }
            if length > 1000 {
                self.remainder_error("type length limit exceeded");
                break;
            }
        }
        if union || intersection {
            self.wrap(
                checkpoint,
                if union {
                    K::UnionType
                } else {
                    K::IntersectionType
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

    #[expect(
        clippy::too_many_lines,
        reason = "type atom dispatch distinguishes context-dependent types and packs"
    )]
    fn type_atom(&mut self, allow_pack: bool) -> TypeForm {
        use SyntaxKind as K;
        let checkpoint = self.checkpoint();
        let mut form = TypeForm::Type;
        match self.nth(0) {
            "..." => {
                self.start(K::VariadicTypePack);
                self.bump();
                self.ty();
                if !allow_pack {
                    self.error("type pack is not allowed in this context");
                }
                self.finish();
                form = TypeForm::Pack;
            }
            "@" | "<" | "(" => {
                let attributed = self.at("@");
                if attributed {
                    self.attributes();
                    if !self.declaration_context {
                        self.error("type attributes require declaration context");
                    }
                }
                let generics = self.at("<");
                if generics {
                    self.generics(false);
                }
                let function = self.function_type_parentheses();
                self.expect("(");
                let mut count = 0;
                let mut tail = false;
                let mut named = false;
                while !self.eof() && !self.at(")") {
                    let before = self.pos;
                    if self.kind() == Some(K::Identifier) && self.nth(1) == ":" {
                        self.name();
                        self.bump();
                        named = true;
                    }
                    if tail {
                        self.error("type pack must be last");
                    }
                    let item = self.type_value(
                        (allow_pack || function) && !matches!(self.nth(0), "@" | "<" | "("),
                    );
                    tail = item == TypeForm::Pack;
                    count += 1;
                    if self.pos == before || !self.eat(",") {
                        break;
                    }
                    if self.at(")") {
                        self.error("expected type after comma");
                        break;
                    }
                }
                self.expect(")");
                if self.at("->") || self.at(":") || generics || named || attributed {
                    self.wrap(checkpoint, K::FunctionType);
                    if self.eat(":") {
                        self.error("function types use '->' for returns");
                    } else {
                        self.expect("->");
                    }
                    self.type_value(true);
                    self.finish();
                } else if allow_pack
                    && !(count == 1 && !tail && matches!(self.nth(0), "|" | "&" | "?"))
                {
                    self.wrap(checkpoint, K::TypePack);
                    self.finish();
                    form = TypeForm::Pack;
                } else {
                    self.wrap(checkpoint, K::TypeGroup);
                    if count != 1 || tail {
                        self.error("type groups require one type; packs require a pack context");
                    }
                    self.finish();
                }
            }
            "{" => {
                self.start(K::TableType);
                self.bump();
                let mut indexer = false;
                let mut first = true;
                while !self.eof() && !self.at("}") {
                    let before = self.pos;
                    let array = self.type_field(&mut indexer, first);
                    first = false;
                    if array {
                        if !self.at("}") {
                            self.error("array shorthand must be the only table type field");
                        }
                        break;
                    }
                    if self.pos == before || !(self.eat(",") || self.eat(";")) {
                        break;
                    }
                }
                self.expect("}");
                self.finish();
            }
            "nil" | "true" | "false" => {
                self.bump();
            }
            _ if self.kind() == Some(K::String) => {
                if self.nth(0).starts_with('`') {
                    self.error("backtick strings cannot be used as types");
                }
                self.literal();
            }
            _ if self.kind() == Some(K::InterpolationStart) => {
                self.interpolation();
                self.error("interpolated strings cannot be used as types");
            }
            _ if self.kind() == Some(K::Identifier) => {
                if self.nth(1) == "..." {
                    self.start(K::GenericTypePack);
                    self.name();
                    self.bump();
                    if !allow_pack {
                        self.error("type pack is not allowed in this context");
                    }
                    self.finish();
                    form = TypeForm::Pack;
                } else if self.at("typeof") && self.nth(1) != "." {
                    self.start(K::TypeofType);
                    self.bump();
                    self.expect("(");
                    self.expression(0);
                    self.expect(")");
                    self.finish();
                } else {
                    self.start(K::TypeName);
                    self.bump();
                    if self.eat(".") {
                        self.name();
                    }
                    self.finish();
                    if self.at("<") {
                        self.type_arguments();
                    }
                }
            }
            _ => {
                self.error("expected type");
                if !self.boundary() && !matches!(self.nth(0), ">" | "=" | "->") {
                    self.bump();
                }
            }
        }
        form
    }

    fn type_arguments(&mut self) {
        self.start(SyntaxKind::TypeArguments);
        self.expect("<");
        if !self.at(">") {
            loop {
                let before = self.pos;
                self.type_value(true);
                if before == self.pos || !self.eat(",") {
                    break;
                }
                if self.at(">") {
                    self.error("expected type argument after comma");
                    break;
                }
            }
        }
        self.expect(">");
        self.finish();
    }

    fn type_field(&mut self, indexer: &mut bool, first: bool) -> bool {
        use SyntaxKind as K;
        self.trivia();
        let checkpoint = self.checkpoint();
        if matches!(self.nth(0), "read" | "write") && self.nth(1) != ":" {
            self.bump();
        }
        if self.eat("[") {
            let string_key = self.kind() == Some(K::String) && self.nth(1) == "]";
            self.wrap(
                checkpoint,
                if string_key {
                    K::TypeProperty
                } else {
                    K::TypeIndexer
                },
            );
            if string_key {
                if let Some(token) = self.current() {
                    let body = &self.bytes[token.start..token.end];
                    if decode_string(body).is_ok_and(|bytes| bytes.contains(&0)) {
                        self.error("property name cannot contain NUL");
                    }
                }
                self.literal();
            } else {
                if *indexer {
                    self.error("only one table indexer is allowed");
                }
                *indexer = true;
                self.ty();
            }
            self.expect("]");
            self.expect(":");
            self.ty();
            self.finish();
            false
        } else if self.kind() == Some(K::Identifier) && self.nth(1) == ":" {
            self.wrap(checkpoint, K::TypeProperty);
            self.name();
            self.bump();
            self.ty();
            self.finish();
            false
        } else if first {
            self.wrap(checkpoint, K::ArrayType);
            self.ty();
            self.finish();
            true
        } else {
            self.wrap(checkpoint, K::TypeProperty);
            self.name();
            self.expect(":");
            self.ty();
            self.finish();
            false
        }
    }
    fn expressions(&mut self) -> (usize, bool) {
        self.trivia();
        self.start(SyntaxKind::ExpressionList);
        let mut last = self.expression(0);
        let mut count = 1;
        while self.eat(",") {
            last = self.expression(0);
            count += 1;
        }
        self.finish();
        (count, last.multiple)
    }

    fn expression(&mut self, limit: u8) -> Expression {
        use SyntaxKind as K;
        let old_depth = self.depth;
        if !self.enter() {
            return Expression::default();
        }
        let previous = self.error_kind;
        self.error_kind = K::ErrorExpression;
        let checkpoint = self.checkpoint();
        let mut result;
        if matches!(self.nth(0), "not" | "-" | "#" | "!") {
            self.start(K::UnaryExpression);
            if self.at("!") {
                self.error("unexpected '!'; use 'not'");
            }
            self.bump();
            self.expression(8);
            self.finish();
            result = Expression::default();
        } else {
            let prefix = self.kind() == Some(K::Identifier) || self.at("(");
            result = self.atom();
            if prefix {
                result = self.postfix(checkpoint, result);
            }
            if self.eat("::") {
                self.wrap(checkpoint, K::TypeAssertionExpression);
                self.ty();
                self.finish();
                result = Expression::default();
            }
        }
        loop {
            let confusable = matches!(
                (self.nth(0), self.nth(1)),
                ("&", "&") | ("|", "|") | ("!", "=")
            );
            let (left, right) = match self.nth(0) {
                "or" => (1, 1),
                "and" => (2, 2),
                "==" | "~=" | "<" | "<=" | ">" | ">=" => (3, 3),
                ".." => (5, 4),
                "+" | "-" => (6, 6),
                "*" | "/" | "//" | "%" => (7, 7),
                "^" => (10, 9),
                "|" if confusable => (1, 1),
                "&" if confusable => (2, 2),
                "!" if confusable => (3, 3),
                _ => break,
            };
            if left <= limit {
                break;
            }
            self.wrap(checkpoint, K::BinaryExpression);
            if confusable {
                self.error("use Luau operators 'and', 'or' and '~='");
                self.bump();
            }
            self.bump();
            self.expression(right);
            self.finish();
            result = Expression::default();
            if !self.enter() {
                break;
            }
        }
        self.depth = old_depth;
        self.error_kind = previous;
        result
    }

    fn postfix(&mut self, checkpoint: Checkpoint, mut result: Expression) -> Expression {
        use SyntaxKind as K;
        loop {
            match self.nth(0) {
                "." => {
                    self.wrap(checkpoint, K::FieldExpression);
                    self.bump();
                    self.name();
                    self.finish();
                    result = Expression {
                        lvalue: true,
                        ..Expression::default()
                    };
                }
                "[" => {
                    self.wrap(checkpoint, K::IndexExpression);
                    self.bump();
                    self.expression(0);
                    self.expect("]");
                    self.finish();
                    result = Expression {
                        lvalue: true,
                        ..Expression::default()
                    };
                }
                "<" if self.nth(1) == "<" => {
                    self.wrap(checkpoint, K::InstantiationExpression);
                    self.bump();
                    self.type_arguments();
                    self.expect(">");
                    self.finish();
                    result = Expression::default();
                }
                _ if matches!(self.nth(0), ":" | "(" | "{") || self.kind() == Some(K::String) => {
                    if self.eat(":") {
                        self.wrap(checkpoint, K::MethodExpression);
                        self.name();
                        self.finish();
                        self.wrap(checkpoint, K::CallExpression);
                        if self.at("<") && self.nth(1) == "<" {
                            self.bump();
                            self.type_arguments();
                            self.expect(">");
                        }
                    } else {
                        self.wrap(checkpoint, K::CallExpression);
                    }
                    if self.at("(") && self.pos > 0 {
                        let end = self.tokens[..self.pos]
                            .iter()
                            .rev()
                            .find(|token| !token.kind.is_trivia())
                            .map_or(0, |token| token.end);
                        if self.text[end..self.start_offset()].contains('\n') {
                            self.error("ambiguous call across newline; use ';' between statements");
                        }
                    }
                    self.arguments();
                    self.finish();
                    result = Expression {
                        call: true,
                        multiple: true,
                        ..Expression::default()
                    };
                }
                _ => break,
            }
            if !self.enter() {
                break;
            }
        }
        result
    }

    fn literal(&mut self) {
        if let Some(token) = self.current() {
            if token.kind == SyntaxKind::String
                && decode_string(&self.bytes[token.start..token.end]).is_err()
            {
                self.error("malformed string escape or literal");
            }
            if token.kind == SyntaxKind::Number && self.text[token.start..token.end].ends_with('i')
            {
                self.feature(Feature::IntegerLiterals, token.start);
            }
        }
        self.bump();
    }

    fn atom(&mut self) -> Expression {
        use SyntaxKind as K;
        let mut result = Expression::default();
        match self.nth(0) {
            "@" | "function" => {
                self.start(K::FunctionExpression);
                if self.at("@") {
                    self.attributes();
                }
                self.expect("function");
                self.function_body();
                self.finish();
            }
            "if" => {
                self.start(K::IfExpression);
                self.bump();
                self.expression(0);
                self.expect("then");
                self.expression(0);
                while self.eat("elseif") {
                    self.expression(0);
                    self.expect("then");
                    self.expression(0);
                }
                self.expect("else");
                self.expression(0);
                self.finish();
            }
            "(" => {
                self.start(K::ParenthesizedExpression);
                self.bump();
                self.expression(0);
                self.expect(")");
                self.finish();
            }
            "{" => self.table(),
            "..." => {
                self.start(K::VarargExpression);
                if !self.vararg {
                    self.error("varargs require a variadic function");
                }
                self.bump();
                self.finish();
                result.multiple = true;
            }
            "nil" | "true" | "false" => {
                self.start(K::LiteralExpression);
                self.bump();
                self.finish();
            }
            _ if self.kind() == Some(K::Identifier) => {
                self.start(K::NameExpression);
                self.bump();
                self.finish();
                result.lvalue = true;
            }
            _ if matches!(self.kind(), Some(K::Number | K::String)) => {
                self.start(K::LiteralExpression);
                self.literal();
                self.finish();
            }
            _ if self.kind() == Some(K::InterpolationStart) => self.interpolation(),
            _ => {
                self.error("expected expression");
                if !self.boundary() {
                    self.bump();
                }
            }
        }
        result
    }
    fn arguments(&mut self) {
        self.start(SyntaxKind::Arguments);
        if self.eat("(") {
            if !self.at(")") {
                self.expressions();
            }
            self.expect(")");
        } else if self.at("{") {
            self.table();
        } else if self.kind() == Some(SyntaxKind::String) {
            if self.nth(0).starts_with('`') {
                self.error("backtick call arguments require parentheses");
            }
            self.atom();
        } else if self.kind() == Some(SyntaxKind::InterpolationStart) {
            self.error("interpolated call arguments require parentheses");
            self.interpolation();
        } else {
            self.error("expected call arguments");
        }
        self.finish();
    }
    fn table(&mut self) {
        self.start(SyntaxKind::TableExpression);
        self.expect("{");
        while !self.eof() && !self.at("}") {
            let before = self.pos;
            self.start(SyntaxKind::TableField);
            if self.eat("[") {
                self.expression(0);
                self.expect("]");
                self.expect("=");
                self.expression(0);
            } else if self.kind() == Some(SyntaxKind::Identifier) && self.nth(1) == "=" {
                self.name();
                self.bump();
                self.expression(0);
            } else {
                self.expression(0);
            }
            self.finish();
            if self.pos == before || !(self.eat(",") || self.eat(";")) {
                break;
            }
        }
        self.expect("}");
        self.finish();
    }
    fn interpolation(&mut self) {
        use SyntaxKind as K;
        self.start(K::InterpolationExpression);
        self.bump();
        while !self.eof() && self.kind() != Some(K::InterpolationEnd) {
            if self.kind() == Some(K::InterpolationText) {
                let token = self.current().expect("interpolation text");
                if quoted_bytes(&self.bytes[token.start..token.end]).is_err() {
                    self.error("malformed interpolation escape");
                }
                self.bump();
            } else if self.kind() == Some(K::InterpolationOpen) {
                self.start(K::InterpolationPart);
                self.bump();
                self.expression(0);
                if self.kind() == Some(K::InterpolationClose) {
                    self.bump();
                } else {
                    self.error("expected interpolation close");
                    self.finish();
                    break;
                }
                self.finish();
            } else {
                break;
            }
        }
        if self.kind() == Some(K::InterpolationEnd) {
            self.bump();
        } else {
            self.error("expected interpolation end");
        }
        self.finish();
    }
}
