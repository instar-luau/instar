use std::io;

use instar_analysis::Location;
use serde::{Deserialize, Serialize};

use super::location;
use crate::boundary;

/// Native completion context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum SuggestionContext {
    /// No classified context.
    Unknown,

    /// Value expression.
    Expression,

    /// Statement position.
    Statement,

    /// Member access.
    Property,

    /// Type expression.
    Type,

    /// Language keyword.
    Keyword,

    /// String contents.
    String,

    /// Analysis directive.
    Comment,
}

impl TryFrom<u8> for SuggestionContext {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Unknown),
            1 => Ok(Self::Expression),
            2 => Ok(Self::Statement),
            3 => Ok(Self::Property),
            4 => Ok(Self::Type),
            5 => Ok(Self::Keyword),
            6 => Ok(Self::String),
            7 => Ok(Self::Comment),
            _ => Err(io::Error::other("unknown native suggestion context")),
        }
    }
}

/// Native completion entry category.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum SuggestionKind {
    /// Accessible member.
    Property,

    /// Lexical or global binding.
    Binding,

    /// Language keyword.
    Keyword,

    /// Contextual string value.
    String,

    /// Type name.
    Type,

    /// Imported module.
    Module,

    /// Generated function expression or parameter list.
    Function,

    /// Import path.
    Path,

    /// Analysis directive.
    Comment,
}

impl TryFrom<u8> for SuggestionKind {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Property),
            1 => Ok(Self::Binding),
            2 => Ok(Self::Keyword),
            3 => Ok(Self::String),
            4 => Ok(Self::Type),
            5 => Ok(Self::Module),
            6 => Ok(Self::Function),
            7 => Ok(Self::Path),
            8 => Ok(Self::Comment),
            _ => Err(io::Error::other("unknown native suggestion kind")),
        }
    }
}

/// Recommended call insertion behavior.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Parentheses {
    /// Preserve the surrounding call syntax.
    None,

    /// Insert an empty call and leave the cursor after it.
    After,

    /// Insert a call with its cursor inside the argument list.
    Inside,
}

impl TryFrom<u8> for Parentheses {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::After),
            2 => Ok(Self::Inside),

            _ => Err(io::Error::other(
                "unknown native parentheses recommendation",
            )),
        }
    }
}

/// Required syntax when inserting a member name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Notation {
    /// Ordinary identifier access.
    Identifier,

    /// Bracketed string access.
    Index,

    /// Colon access with an implicit receiver.
    Method,
}

impl TryFrom<u8> for Notation {
    type Error = io::Error;

    fn try_from(value: u8) -> io::Result<Self> {
        match value {
            0 => Ok(Self::Identifier),
            1 => Ok(Self::Index),
            2 => Ok(Self::Method),
            _ => Err(io::Error::other("unknown native member notation")),
        }
    }
}

/// A detached completion entry.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Suggestion {
    /// Display and filter name.
    pub name: String,

    /// Native entry category.
    pub kind: SuggestionKind,

    /// Detail supplied by the completion provider.
    pub detail: String,

    /// Qualified documentation identity when available.
    pub documentation: Option<String>,

    /// Explicit insertion text supplied by the native provider.
    pub insert_text: Option<String>,

    /// Native call insertion recommendation.
    pub parentheses: Parentheses,

    /// Whether the entry is deprecated.
    pub deprecated: bool,

    /// Whether the value has a callable type.
    pub callable: bool,

    /// Whether the entry matches the expected type.
    pub preferred: bool,

    /// Required member insertion syntax.
    pub notation: Notation,
}

impl TryFrom<boundary::NativeSuggestion> for Suggestion {
    type Error = io::Error;

    fn try_from(value: boundary::NativeSuggestion) -> io::Result<Self> {
        Ok(Self {
            name: value.name,
            kind: value.kind.try_into()?,
            detail: value.detail,
            documentation: (!value.documentation.is_empty()).then_some(value.documentation),
            insert_text: (!value.insert_text.is_empty()).then_some(value.insert_text),
            parentheses: value.parentheses.try_into()?,
            deprecated: value.deprecated,
            callable: value.callable,
            preferred: value.preferred,
            notation: value.notation.try_into()?,
        })
    }
}

/// Revision-anchored completion entries and their replacement context.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Suggestions<Module = String> {
    /// Source range to replace.
    pub location: Location<Module>,

    /// Native syntactic context.
    pub context: SuggestionContext,

    /// Preceding member operator byte range when available.
    pub index: Option<[usize; 2]>,

    /// Whether a string argument belongs to the native require function.
    pub require: bool,

    /// Whether the entries expose a canonical require binding.
    pub loader: bool,

    /// Decoded string contents before the cursor, when available.
    pub prefix: Option<String>,

    /// Entries in deterministic name order.
    pub entries: Vec<Suggestion>,
}

impl TryFrom<boundary::NativeSuggestions> for Suggestions {
    type Error = io::Error;

    fn try_from(value: boundary::NativeSuggestions) -> io::Result<Self> {
        Ok(Self {
            location: location(value.location),
            context: value.context.try_into()?,
            index: value
                .has_index
                .then_some([value.index_start, value.index_end]),
            require: value.require,
            loader: value.loader,
            prefix: value.has_prefix.then_some(value.prefix),
            entries: value
                .entries
                .into_iter()
                .map(Suggestion::try_from)
                .collect::<io::Result<_>>()?,
        })
    }
}
