mod kind;
mod lexer;
mod parser;

pub use kind::{Luau, SyntaxKind};
pub use parser::{
    EntryPoint, Feature, FeatureUse, HotComment, NumberLiteral, NumberStatus, NumberValue, Parse,
    ParseError, ParseOptions, StringLiteralError, number_value, string_bytes,
};

pub type SyntaxNode = rowan::SyntaxNode<Luau>;
pub type SyntaxToken = rowan::SyntaxToken<Luau>;
pub type SyntaxElement = rowan::SyntaxElement<Luau>;
