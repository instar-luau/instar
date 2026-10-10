//! Source token classes combined with native binding and type identities.

use std::{collections::BTreeMap, io};

use instar_shared::{Options, error::invalid};

use instar_syntax::{
    token::{Span, Symbol, TokenKind as SyntaxTokenKind},
    tree::{NodeKind, Tree},
};

/// Language role of a source token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Source comment.
    Comment,

    /// Reserved or contextual keyword.
    Keyword,

    /// String literal or interpolation text.
    String,

    /// Numeric literal.
    Number,

    /// Value or type operator.
    Operator,

    /// Function attribute.
    Decorator,

    /// Type declaration or reference.
    Type,

    /// Generic type parameter.
    TypeParameter,

    /// Value binding.
    Variable,

    /// Function parameter or implicit receiver.
    Parameter,

    /// Function-valued binding.
    Function,

    /// Named member.
    Property,

    /// Function-valued member.
    Method,
}

/// Additional semantic property of a source occurrence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenModifier {
    /// Declares a binding or member.
    Declaration,

    /// Modifies a binding or member.
    Modification,

    /// Native builtin without a source declaration.
    DefaultLibrary,

    /// Deprecated according to native metadata.
    Deprecated,
}

/// One nonoverlapping token in the enclosing source snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    /// Half-open UTF-8 byte range.
    pub range: [usize; 2],

    /// Language role.
    pub kind: TokenKind,

    /// Additional native properties of the occurrence.
    pub modifiers: Vec<TokenModifier>,
}

pub(crate) fn tokens(
    source: &instar_shared::SourceSnapshot,
    settings: &instar_syntax::SyntaxSettings,
    facts: Option<&instar_luau::FactsResult>,
    options: &Options,
) -> io::Result<Vec<Token>> {
    let parsed = instar_syntax::parse(source, settings, options);
    let mut tokens = syntax(parsed.tree.syntax(), options)?;

    if let Some(facts) = facts {
        for occurrence in &facts.facts.occurrences {
            options.check()?;

            if occurrence.location.source != source.version {
                continue;
            }

            let range = [occurrence.location.span.start, occurrence.location.span.end];
            occurrence.location.validate(source)?;

            let kind = match occurrence.role {
                instar_luau::OccurrenceRole::Variable => TokenKind::Variable,
                instar_luau::OccurrenceRole::Parameter => TokenKind::Parameter,
                instar_luau::OccurrenceRole::Function => TokenKind::Function,
                instar_luau::OccurrenceRole::Property => TokenKind::Property,
                instar_luau::OccurrenceRole::Method => TokenKind::Method,
                instar_luau::OccurrenceRole::Type => TokenKind::Type,
            };

            let builtin = matches!(&occurrence.targets, instar_shared::Knowledge::Known(targets) if targets.iter().any(|target| matches!(target, instar_luau::SemanticIdentity::Environment { .. })));

            let modifiers = [
                (TokenModifier::Declaration, occurrence.declaration),
                (
                    TokenModifier::Modification,
                    occurrence.access.writes() && !occurrence.declaration,
                ),
                (TokenModifier::DefaultLibrary, builtin),
                (TokenModifier::Deprecated, occurrence.deprecated),
            ]
            .into_iter()
            .filter_map(|(modifier, present)| present.then_some(modifier))
            .collect();

            tokens.insert(
                range,
                Token {
                    range,
                    kind,
                    modifiers,
                },
            );
        }
    }

    let tokens = tokens.into_values().collect::<Vec<_>>();

    if tokens
        .windows(2)
        .any(|pair| pair[0].range[1] > pair[1].range[0])
    {
        return Err(invalid("semantic token ranges overlap"));
    }

    Ok(tokens)
}

fn syntax(tree: &Tree<'_>, options: &Options) -> io::Result<BTreeMap<[usize; 2], Token>> {
    let mut tokens = BTreeMap::new();

    for token in &tree.tokens {
        options.check()?;

        let kind = match token.kind {
            SyntaxTokenKind::Comment
            | SyntaxTokenKind::BlockComment
            | SyntaxTokenKind::MalformedComment => TokenKind::Comment,

            SyntaxTokenKind::Keyword(_) => TokenKind::Keyword,

            SyntaxTokenKind::RawString
            | SyntaxTokenKind::QuotedString
            | SyntaxTokenKind::InterpolatedStringStart
            | SyntaxTokenKind::InterpolatedStringMiddle
            | SyntaxTokenKind::InterpolatedStringEnd
            | SyntaxTokenKind::InterpolatedStringSimple
            | SyntaxTokenKind::MalformedString => TokenKind::String,

            SyntaxTokenKind::Number => TokenKind::Number,
            SyntaxTokenKind::Attribute => TokenKind::Decorator,
            SyntaxTokenKind::Symbol(symbol) if operator(symbol) => TokenKind::Operator,
            _ => continue,
        };

        append(token.span, kind, &mut tokens);
    }

    let mut pending = vec![tree.root];

    while let Some(node) = pending.pop() {
        options.check()?;

        match &tree.node(node).kind {
            NodeKind::TypeAlias { keyword, .. }
            | NodeKind::TypeOf { keyword, .. }
            | NodeKind::Continue { keyword }
            | NodeKind::Constant { keyword, .. }
            | NodeKind::Export { keyword, .. }
            | NodeKind::Declaration { keyword, .. }
            | NodeKind::Extends { keyword, .. } => {
                append(tree.token(*keyword).span, TokenKind::Keyword, &mut tokens);
            }

            NodeKind::TypeName { name, .. } => {
                append(tree.node(*name).span, TokenKind::Type, &mut tokens);
            }

            NodeKind::Generic { name, .. } | NodeKind::GenericPack { name, .. } => {
                append(tree.node(*name).span, TokenKind::TypeParameter, &mut tokens);
            }

            _ => {}
        }

        pending.extend(tree.children(node));
    }

    Ok(tokens)
}

fn append(span: Span, kind: TokenKind, tokens: &mut BTreeMap<[usize; 2], Token>) {
    if span.start == span.end {
        return;
    }

    let range = [span.start, span.end];

    tokens.insert(
        range,
        Token {
            range,
            kind,
            modifiers: Vec::new(),
        },
    );
}

fn operator(symbol: Symbol) -> bool {
    matches!(
        symbol,
        Symbol::Add
            | Symbol::Subtract
            | Symbol::Multiply
            | Symbol::Divide
            | Symbol::FloorDivide
            | Symbol::Modulo
            | Symbol::Power
            | Symbol::Length
            | Symbol::Concatenate
            | Symbol::Assignment
            | Symbol::Equal
            | Symbol::NotEqual
            | Symbol::LessThan
            | Symbol::LessThanOrEqual
            | Symbol::GreaterThan
            | Symbol::GreaterThanOrEqual
            | Symbol::AddAssignment
            | Symbol::SubtractAssignment
            | Symbol::MultiplyAssignment
            | Symbol::DivideAssignment
            | Symbol::FloorDivideAssignment
            | Symbol::ModuloAssignment
            | Symbol::PowerAssignment
            | Symbol::ConcatenateAssignment
            | Symbol::Arrow
            | Symbol::QuestionMark
            | Symbol::Ampersand
            | Symbol::Pipe
            | Symbol::Tilde
            | Symbol::DoubleColon
    )
}
