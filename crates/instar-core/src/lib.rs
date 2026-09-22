//! Project configuration and module graphs for Instar.

pub mod analysis;
pub mod config;
pub mod graph;
pub mod project;
pub mod resolve;
pub mod roblox;

use std::{
    io,
    path::{Component, Path, PathBuf},
};

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// Lexical identity preserves symlink spelling, matching Luau's filesystem navigator.
fn absolute(path: &Path) -> io::Result<PathBuf> {
    let path = std::path::absolute(path)?;
    let mut result = PathBuf::new();

    for part in path.components() {
        match part {
            Component::CurDir => {}

            Component::ParentDir => {
                result.pop();
            }

            other => result.push(other.as_os_str()),
        }
    }

    Ok(result)
}

fn string_value(bytes: &[u8]) -> io::Result<String> {
    if bytes.first() == Some(&b'[') {
        return long_string(bytes);
    }

    if bytes.len() < 2 || !matches!(bytes[0], b'\'' | b'"') || bytes.last() != bytes.first() {
        return Err(invalid("expected a string literal"));
    }

    let mut output = Vec::new();
    let mut index = 1;

    while index < bytes.len() - 1 {
        let byte = bytes[index];
        index += 1;

        if byte != b'\\' {
            output.push(byte);
            continue;
        }

        let byte = *bytes
            .get(index)
            .ok_or_else(|| invalid("incomplete string escape"))?;

        index += 1;

        match byte {
            b'a' => output.push(7),
            b'b' => output.push(8),
            b'f' => output.push(12),
            b'n' => output.push(b'\n'),
            b'r' => output.push(b'\r'),
            b't' => output.push(b'\t'),
            b'v' => output.push(11),
            b'\\' | b'\'' | b'"' => output.push(byte),

            b'\n' | b'\r' => {
                if bytes
                    .get(index)
                    .is_some_and(|&next| matches!(next, b'\r' | b'\n') && next != byte)
                {
                    index += 1;
                }

                output.push(b'\n');
            }

            b'z' => {
                while bytes.get(index).is_some_and(u8::is_ascii_whitespace) {
                    index += 1;
                }
            }

            b'x' => {
                let digits = bytes
                    .get(index..index + 2)
                    .ok_or_else(|| invalid("invalid hexadecimal escape"))?;

                let value = u8::from_str_radix(
                    std::str::from_utf8(digits).map_err(|e| invalid(e.to_string()))?,
                    16,
                )
                .map_err(|e| invalid(e.to_string()))?;

                output.push(value);
                index += 2;
            }

            b'u' => {
                if bytes.get(index) != Some(&b'{') {
                    return Err(invalid("invalid Unicode escape"));
                }

                index += 1;

                let end = bytes[index..]
                    .iter()
                    .position(|&b| b == b'}')
                    .map(|n| n + index)
                    .ok_or_else(|| invalid("invalid Unicode escape"))?;

                let digits =
                    std::str::from_utf8(&bytes[index..end]).map_err(|e| invalid(e.to_string()))?;

                let value = u32::from_str_radix(digits, 16)
                    .ok()
                    .and_then(char::from_u32)
                    .ok_or_else(|| invalid("invalid Unicode codepoint"))?;

                let mut encoded = [0; 4];
                output.extend_from_slice(value.encode_utf8(&mut encoded).as_bytes());
                index = end + 1;
            }

            b'0'..=b'9' => {
                let mut value = u16::from(byte - b'0');

                for _ in 0..2 {
                    if let Some(&digit @ b'0'..=b'9') = bytes.get(index) {
                        value = value * 10 + u16::from(digit - b'0');
                        index += 1;
                    } else {
                        break;
                    }
                }

                output
                    .push(u8::try_from(value).map_err(|_| invalid("decimal escape exceeds 255"))?);
            }

            _ => return Err(invalid("unknown string escape")),
        }
    }

    String::from_utf8(output).map_err(invalid_utf8)
}

fn invalid_utf8(error: std::string::FromUtf8Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error)
}

fn long_string(bytes: &[u8]) -> io::Result<String> {
    let equals = bytes.iter().skip(1).take_while(|&&b| b == b'=').count();
    let width = equals + 2;

    if bytes.get(width - 1) != Some(&b'[') || bytes.len() < width * 2 {
        return Err(invalid("invalid long string"));
    }

    let closing = &bytes[bytes.len() - width..];

    if closing[0] != b']'
        || closing[width - 1] != b']'
        || closing[1..width - 1].iter().any(|&b| b != b'=')
    {
        return Err(invalid("invalid long string delimiter"));
    }

    let body = &bytes[width..bytes.len() - width];

    let body = body
        .strip_prefix(b"\r\n")
        .or_else(|| body.strip_prefix(b"\n\r"))
        .or_else(|| body.strip_prefix(b"\n"))
        .or_else(|| body.strip_prefix(b"\r"))
        .unwrap_or(body);

    let mut output = Vec::with_capacity(body.len());
    let mut index = 0;

    while index < body.len() {
        let byte = body[index];
        index += 1;

        if matches!(byte, b'\r' | b'\n') {
            if body
                .get(index)
                .is_some_and(|&next| matches!(next, b'\r' | b'\n') && next != byte)
            {
                index += 1;
            }

            output.push(b'\n');
        } else {
            output.push(byte);
        }
    }

    String::from_utf8(output).map_err(invalid_utf8)
}
