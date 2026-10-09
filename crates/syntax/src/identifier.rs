use vermis::token::TokenKind;

/// Whether the entire source is one Luau identifier rather than a keyword.
#[must_use]
pub fn identifier(name: &str) -> bool {
    let tree = vermis::parse(name.as_bytes());

    tree.tokens.first().is_some_and(|token| {
        token.kind == TokenKind::Name && token.span.start == 0 && token.span.end == name.len()
    })
}
