//! Luau literal decoding and interpolation boundaries.

use std::{fmt::Write, str};

use instar_shared::Span;

use vermis::{
    token::TokenKind,
    tree::{NodeIndex, NodeKind, Tree},
};

/// A decoded numeric representation, distinguishing integers from floating numbers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberValue {
    /// Integer two's-complement bits.
    Integer(u64),

    /// IEEE-754 double-precision bits.
    Float(u64),
}

/// One interpolation segment or expression with original byte boundaries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterpolationPart {
    /// Exact original segment or expression span.
    pub span: Span,

    /// Decoded segment bytes; absent for an embedded expression.
    pub value: Option<Vec<u8>>,
}

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

/// Encodes UTF-8 text as a quoted Luau string literal.
#[must_use]
pub fn quote(value: &str) -> String {
    let mut text = String::from("\"");

    for character in value.chars() {
        match character {
            '\\' => text.push_str("\\\\"),
            '"' => text.push_str("\\\""),

            character if character.is_ascii_control() => {
                write!(text, "\\{:03}", u32::from(character)).expect("string write");
            }

            character => text.push(character),
        }
    }

    text.push('"');

    text
}

/// Decodes a quoted or long-bracket spelling, retaining byte-valued escapes.
#[must_use]
pub fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    str::from_utf8(bytes).ok()?;
    let first = *bytes.first()?;

    if first == b'[' {
        return long(bytes);
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
            if byte == first || byte == 0 || first != b'`' && matches!(byte, b'\r' | b'\n') {
                return None;
            }

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
                while content
                    .get(index)
                    .is_some_and(|byte| matches!(*byte, b' ' | b'\t' | b'\r' | b'\n' | 0x0b | 0x0c))
                {
                    index += 1;
                }
            }

            b'x' => {
                let digits = str::from_utf8(content.get(index..index + 2)?).ok()?;
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

                let digits = str::from_utf8(&content[index..end]).ok()?;
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
                    str::from_utf8(&content[start..index])
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

/// Decodes a supported numeric node without collapsing integer and floating representations.
#[must_use]
pub fn number(tree: &Tree<'_>, node: NodeIndex) -> Option<NumberValue> {
    if !matches!(tree.node(node).kind, NodeKind::Number { .. }) {
        return None;
    }

    let spelling = str::from_utf8(tree.text(node)).ok()?.replace('_', "");
    let integer = spelling.ends_with('i');
    let spelling = spelling.strip_suffix('i').unwrap_or(&spelling);

    let radix = if spelling.starts_with("0x") || spelling.starts_with("0X") {
        16
    } else if spelling.starts_with("0b") || spelling.starts_with("0B") {
        2
    } else {
        10
    };

    if radix != 10 {
        let value = u64::from_str_radix(&spelling[2..], radix).ok()?;

        if integer {
            Some(NumberValue::Integer(value))
        } else {
            Some(NumberValue::Float(
                value.to_string().parse::<f64>().ok()?.to_bits(),
            ))
        }
    } else if integer {
        Some(NumberValue::Integer(u64::from_ne_bytes(
            spelling.parse::<i64>().ok()?.to_ne_bytes(),
        )))
    } else {
        Some(NumberValue::Float(spelling.parse::<f64>().ok()?.to_bits()))
    }
}

/// Decodes interpolation segments while retaining every embedded-expression boundary.
#[must_use]
pub fn interpolation(tree: &Tree<'_>, node: NodeIndex) -> Option<Vec<InterpolationPart>> {
    let indices = match &tree.node(node).kind {
        NodeKind::Interpolation { segments } => tree
            .list(segments)
            .iter()
            .map(|entry| entry.node)
            .collect::<Vec<_>>(),

        NodeKind::String { token }
            if tree.token(*token).kind == TokenKind::InterpolatedStringSimple =>
        {
            vec![node]
        }

        _ => return None,
    };

    indices
        .into_iter()
        .map(|index| {
            let node = tree.node(index);

            if matches!(node.kind, NodeKind::Error | NodeKind::Missing { .. }) {
                return None;
            }

            let value = if let NodeKind::String { token } = &node.kind {
                if !matches!(
                    tree.token(*token).kind,
                    TokenKind::InterpolatedStringStart
                        | TokenKind::InterpolatedStringMiddle
                        | TokenKind::InterpolatedStringEnd
                        | TokenKind::InterpolatedStringSimple
                ) {
                    return None;
                }

                let bytes = tree.text(index);
                let mut quoted = vec![b'`'];
                quoted.extend_from_slice(bytes.get(1..bytes.len().checked_sub(1)?)?);
                quoted.push(b'`');

                Some(decode(&quoted)?)
            } else {
                None
            };

            Some(InterpolationPart {
                span: Span {
                    start: node.span.start,
                    end: node.span.end,
                },
                value,
            })
        })
        .collect()
}

fn long(bytes: &[u8]) -> Option<Vec<u8>> {
    let opening = 2 + bytes.get(1..)?.iter().position(|&byte| byte != b'=')?;
    let closing = opening;
    let mut content = bytes.get(opening..bytes.len().checked_sub(closing)?)?;

    if bytes.get(opening - 1) != Some(&b'[') {
        return None;
    }

    let delimiter = bytes.get(bytes.len().checked_sub(closing)?..)?;

    if delimiter.first() != Some(&b']')
        || delimiter.last() != Some(&b']')
        || !delimiter[1..closing - 1].iter().all(|byte| *byte == b'=')
        || content.windows(closing).any(|window| window == delimiter)
    {
        return None;
    }

    if content.starts_with(b"\r\n") {
        content = &content[2..];
    } else if content.starts_with(b"\n") || content.starts_with(b"\r") {
        content = &content[1..];
    }

    let text = str::from_utf8(content).ok()?;

    Some(text.replace("\r\n", "\n").replace('\r', "\n").into_bytes())
}
