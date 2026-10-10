use std::io;

use instar_shared::{Completion, Diagnostic, Location};
use serde::{Deserialize, Serialize};

use super::semantics::AnalysisIdentity;

/// Native completion context.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum CompletionContext {
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

impl TryFrom<u8> for CompletionContext {
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
pub enum CompletionKind {
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

impl TryFrom<u8> for CompletionKind {
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
pub struct CompletionCandidate {
    /// Display and filter name.
    pub name: String,

    /// Native entry category.
    pub kind: CompletionKind,

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

/// Owned completion candidates and their exact source replacement range.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Completions {
    /// The exact logical analysis identity or guarded reference.
    pub analysis: AnalysisIdentity,

    /// The exact captured range replaced by the completion.
    pub replacement: Location,

    /// The established completion syntax context.
    pub context: CompletionContext,

    /// Detached completion candidates in name order.
    pub candidates: Vec<CompletionCandidate>,

    /// The member operator range when supplied.
    pub operator: Option<Location>,

    /// Whether the cursor is an established loader string context.
    pub require: bool,

    /// Whether completion identified a lexical loader context.
    pub loader: bool,

    /// The decoded import string prefix when established.
    pub prefix: Option<String>,

    /// Detached revision-anchored findings.
    pub diagnostics: Vec<Diagnostic>,

    /// Coverage and interruption outcome independent of source errors.
    pub completion: Completion,
}
