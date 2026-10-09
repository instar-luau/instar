//! Source token classes combined with native binding and type identities.

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    time::Instant,
};

use instar_analysis::{Location, Options, error::invalid};
use instar_bridge::frontend::Role;

use vermis::{
    token::{Span, Symbol, TokenKind},
    tree::{NodeKind, Tree},
};

use crate::{
    analysis::{Entry, Origin},
    project::Project,
};

/// Language role of a source token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
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
pub enum Modifier {
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
    pub kind: Kind,

    /// Additional native properties of the occurrence.
    pub modifiers: Vec<Modifier>,
}

/// Classified tokens from one exact source and module context.
pub struct Tokens {
    /// Whole-source location anchoring all token ranges.
    pub location: Location<Origin>,

    /// Tokens in source order.
    pub entries: Vec<Token>,
}

impl Project {
    /// Classifies lexical tokens and native references in an explicit module context.
    ///
    /// # Errors
    /// Returns source, configuration, semantic preparation, source agreement, or interruption failures.
    pub fn tokens(&mut self, entry: &Entry, options: &Options) -> io::Result<Tokens> {
        let started = Instant::now();

        self.during(
            |project| {
                let source = project.source(&entry.source)?;
                let tree = vermis::parse(source.text.as_bytes());
                let mut tokens = syntax(&tree, options, started)?;

                let (origin, references) =
                    project.reference_index(entry, &mut BTreeSet::new(), options, started)?;

                for reference in references {
                    options.check(started)?;

                    if reference.location.module != origin || reference.literal {
                        continue;
                    }

                    let range = reference.location.range;

                    if reference.location.revision != source.revision
                        || source.text.get(range[0]..range[1]) != Some(reference.name.as_str())
                    {
                        return Err(invalid("semantic token source revision mismatch"));
                    }

                    let kind = match reference.classification.role {
                        Role::Type => Kind::Type,
                        Role::Parameter => Kind::Parameter,
                        Role::Method => Kind::Method,
                        Role::Property => Kind::Property,
                        Role::Function => Kind::Function,
                        Role::Variable => Kind::Variable,
                    };

                    let modifiers = [
                        (Modifier::Declaration, reference.declaration),
                        (
                            Modifier::Modification,
                            reference.write && !reference.declaration,
                        ),
                        (Modifier::DefaultLibrary, reference.symbol.is_some()),
                        (Modifier::Deprecated, reference.classification.deprecated),
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

                let entries = tokens.into_values().collect::<Vec<_>>();

                if entries
                    .windows(2)
                    .any(|pair| pair[0].range[1] > pair[1].range[0])
                {
                    return Err(invalid("semantic token ranges overlap"));
                }

                options.check(started)?;

                Ok(Tokens {
                    location: Location {
                        module: origin,
                        revision: source.revision,
                        range: [0, source.text.len()],
                    },
                    entries,
                })
            },
            options,
        )
    }
}

fn syntax(
    tree: &Tree<'_>,
    options: &Options,
    started: Instant,
) -> io::Result<BTreeMap<[usize; 2], Token>> {
    let mut tokens = BTreeMap::new();

    for token in &tree.tokens {
        options.check(started)?;

        let kind = match token.kind {
            TokenKind::Comment | TokenKind::BlockComment | TokenKind::MalformedComment => {
                Kind::Comment
            }

            TokenKind::Keyword(_) => Kind::Keyword,

            TokenKind::RawString
            | TokenKind::QuotedString
            | TokenKind::InterpolatedStringStart
            | TokenKind::InterpolatedStringMiddle
            | TokenKind::InterpolatedStringEnd
            | TokenKind::InterpolatedStringSimple
            | TokenKind::MalformedString => Kind::String,

            TokenKind::Number => Kind::Number,
            TokenKind::Attribute => Kind::Decorator,
            TokenKind::Symbol(symbol) if operator(symbol) => Kind::Operator,
            _ => continue,
        };

        append(token.span, kind, &mut tokens);
    }

    let mut pending = vec![tree.root];

    while let Some(node) = pending.pop() {
        options.check(started)?;

        match &tree.node(node).kind {
            NodeKind::TypeAlias { keyword, .. }
            | NodeKind::TypeOf { keyword, .. }
            | NodeKind::Continue { keyword }
            | NodeKind::Constant { keyword, .. }
            | NodeKind::Export { keyword, .. }
            | NodeKind::Declaration { keyword, .. }
            | NodeKind::Extends { keyword, .. } => {
                append(tree.token(*keyword).span, Kind::Keyword, &mut tokens);
            }

            NodeKind::TypeName { name, .. } => {
                append(tree.node(*name).span, Kind::Type, &mut tokens);
            }

            NodeKind::Generic { name, .. } | NodeKind::GenericPack { name, .. } => {
                append(tree.node(*name).span, Kind::TypeParameter, &mut tokens);
            }

            _ => {}
        }

        pending.extend(tree.children(node));
    }

    Ok(tokens)
}

fn append(span: Span, kind: Kind, tokens: &mut BTreeMap<[usize; 2], Token>) {
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
