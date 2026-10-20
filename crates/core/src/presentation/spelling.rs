use std::{fmt::Write as _, str};

use vermis::{lexer::Lexer, token::TokenKind};

use crate::{
    format::Error,
    syntax::{is_identifier, literal},
};

pub(super) fn identifier(name: &str) -> Result<&str, Error> {
    if is_identifier(name) {
        Ok(name)
    } else {
        Err(Error::internal(
            0..0,
            "Presentation contains an invalid identifier.",
        ))
    }
}

pub(super) fn qualified(
    name: &str,
    qualifier: Option<&str>,
    is_verified: bool,
) -> Result<String, Error> {
    if is_verified {
        identifier(name)?;

        if let Some(qualifier) = qualifier {
            identifier(qualifier)?;
        }
    }

    let name = if is_verified {
        name.to_owned()
    } else {
        name.escape_debug().to_string()
    };

    Ok(match qualifier {
        Some(qualifier) => format!(
            "{}.{}",
            if is_verified {
                qualifier.to_owned()
            } else {
                qualifier.escape_debug().to_string()
            },
            name
        ),

        None => name,
    })
}

pub(super) fn property_name(bytes: &[u8]) -> Result<(String, bool), Error> {
    if let Ok(name) = str::from_utf8(bytes)
        && identifier(name).is_ok()
    {
        Ok((name.to_owned(), true))
    } else {
        Ok((format!("[{}]", quoted(bytes)?), false))
    }
}

pub(super) fn quoted(bytes: &[u8]) -> Result<String, Error> {
    let mut output = String::with_capacity(bytes.len() + 2);
    output.push('"');
    let mut remaining = bytes;

    while !remaining.is_empty() {
        match str::from_utf8(remaining) {
            Ok(text) => {
                for character in text.chars() {
                    quote_character(&mut output, character)?;
                }

                remaining = &[];
            }

            Err(error) => {
                let valid = str::from_utf8(&remaining[..error.valid_up_to()]).map_err(|error| {
                    Error::internal(0..0, format!("Cannot preserve singleton bytes. {error}"))
                })?;

                for character in valid.chars() {
                    quote_character(&mut output, character)?;
                }

                let length = error
                    .error_len()
                    .unwrap_or(remaining.len() - error.valid_up_to());

                for byte in &remaining[error.valid_up_to()..error.valid_up_to() + length] {
                    write!(output, "\\{byte:03}")
                        .map_err(|error| Error::internal(0..0, error.to_string()))?;
                }

                remaining = &remaining[error.valid_up_to() + length..];
            }
        }
    }

    output.push('"');
    let mut tokens = Lexer::new(output.as_bytes());

    if !matches!(tokens.next(), Some(token) if token.kind == TokenKind::QuotedString && token.span.end == output.len())
        || !matches!(tokens.next(), Some(token) if token.kind == TokenKind::EndOfFile)
        || literal::decode(output.as_bytes()).as_deref() != Some(bytes)
    {
        return Err(Error::internal(
            0..0,
            "Presentation changed singleton bytes.",
        ));
    }

    Ok(output)
}

fn quote_character(output: &mut String, character: char) -> Result<(), Error> {
    match character {
        '"' => output.push_str("\\\""),
        '\\' => output.push_str("\\\\"),

        character if character.is_control() || matches!(character, '\u{2028}' | '\u{2029}') => {
            let mut bytes = [0; 4];

            for byte in character.encode_utf8(&mut bytes).as_bytes() {
                write!(output, "\\{byte:03}")
                    .map_err(|error| Error::internal(0..0, error.to_string()))?;
            }
        }

        character => output.push(character),
    }

    Ok(())
}
