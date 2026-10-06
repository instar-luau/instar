//! Typed Luau string literal values.

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

/// Decodes a typed quoted or long-bracket string node into its Luau value.
#[must_use]
pub fn string(tree: &Tree<'_>, node: NodeIndex) -> Option<String> {
    let NodeKind::String { token } = &tree.node(node).kind else {
        return None;
    };

    if !matches!(
        tree.token(*token).kind,
        TokenKind::QuotedString | TokenKind::RawString
    ) {
        return None;
    }

    String::from_utf8(decode(tree.text(node))?).ok()
}

/// Decodes literal bytes, including valid non-UTF-8 byte escapes.
#[must_use]
pub fn bytes(tree: &Tree<'_>, node: NodeIndex) -> Option<Vec<u8>> {
    let NodeKind::String { token } = &tree.node(node).kind else {
        return None;
    };

    if !matches!(
        tree.token(*token).kind,
        TokenKind::QuotedString | TokenKind::RawString | TokenKind::InterpolatedStringSimple
    ) {
        return None;
    }

    decode(tree.text(node))
}

fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    let first = *bytes.first()?;

    if first == b'[' {
        let opening = 2 + bytes.get(1..)?.iter().position(|&byte| byte != b'=')?;
        let closing = opening;
        let mut content = bytes.get(opening..bytes.len().checked_sub(closing)?)?;

        if content.starts_with(b"\r\n") {
            content = &content[2..];
        } else if content.starts_with(b"\n") || content.starts_with(b"\r") {
            content = &content[1..];
        }

        let text = std::str::from_utf8(content).ok()?;

        return Some(text.replace("\r\n", "\n").replace('\r', "\n").into_bytes());
    }

    if !matches!(first, b'\'' | b'"' | b'`') || bytes.last() != Some(&first) {
        return None;
    }

    let content = bytes.get(1..bytes.len().checked_sub(1)?)?;
    let mut output = Vec::new();
    let mut index = 0;

    while index < content.len() {
        let byte = content[index];
        index += 1;

        if byte != b'\\' {
            output.push(byte);
            continue;
        }

        let escape = *content.get(index)?;
        index += 1;

        match escape {
            b'a' => output.push(7),
            b'b' => output.push(8),
            b'f' => output.push(12),
            b'n' | b'\n' => output.push(b'\n'),
            b'r' => output.push(b'\r'),
            b't' => output.push(b'\t'),
            b'v' => output.push(11),
            b'\\' | b'\'' | b'"' => output.push(escape),
            b'`' | b'{' if first == b'`' => output.push(escape),

            b'\r' => {
                if content.get(index) == Some(&b'\n') {
                    index += 1;
                }

                output.push(b'\n');
            }

            b'z' => {
                while content.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
            }

            b'x' => {
                let digits = std::str::from_utf8(content.get(index..index + 2)?).ok()?;
                output.push(u8::from_str_radix(digits, 16).ok()?);
                index += 2;
            }

            b'u' => {
                if content.get(index) != Some(&b'{') {
                    return None;
                }

                index += 1;

                let end = index
                    + content
                        .get(index..)?
                        .iter()
                        .position(|&byte| byte == b'}')?;

                let digits = std::str::from_utf8(&content[index..end]).ok()?;
                let character = char::from_u32(u32::from_str_radix(digits, 16).ok()?)?;
                let mut buffer = [0; 4];
                output.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                index = end + 1;
            }

            b'0'..=b'9' => {
                let start = index - 1;

                while index < start + 3 && content.get(index).is_some_and(u8::is_ascii_digit) {
                    index += 1;
                }

                output.push(
                    std::str::from_utf8(&content[start..index])
                        .ok()?
                        .parse::<u8>()
                        .ok()?,
                );
            }

            _ => return None,
        }
    }

    Some(output)
}
