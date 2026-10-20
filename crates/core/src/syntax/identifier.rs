use vermis::{lexer::Lexer, token::TokenKind};

/// Returns whether the entire string is a lexical identifier rather than a keyword.
#[must_use]
pub fn is_identifier(name: &str) -> bool {
    let mut tokens = Lexer::new(name.as_bytes());

    matches!(tokens.next(), Some(token) if token.kind == TokenKind::Name && token.span.start == 0 && token.span.end == name.len())
        && matches!(tokens.next(), Some(token) if token.kind == TokenKind::EndOfFile)
}
