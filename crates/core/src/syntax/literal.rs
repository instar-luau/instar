use std::str;

pub(crate) fn decode(bytes: &[u8]) -> Option<Vec<u8>> {
    str::from_utf8(bytes).ok()?;

    if bytes.contains(&0) {
        return None;
    }

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
            if byte == first || matches!(byte, b'\r' | b'\n') || first == b'`' && byte == b'{' {
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
                let first = char::from(*content.get(index)?).to_digit(16)?;
                let second = char::from(*content.get(index + 1)?).to_digit(16)?;
                output.push(u8::try_from(first * 16 + second).ok()?);
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
                        .position(|byte| *byte == b'}')?;

                let digits = &content[index..end];

                if digits.is_empty() || digits.len() > 16 {
                    return None;
                }

                let character = digits.iter().try_fold(0_u32, |value, digit| {
                    Some(
                        value
                            .wrapping_mul(16)
                            .wrapping_add(char::from(*digit).to_digit(16)?),
                    )
                })?;

                match character {
                    0..=0x7f => output.push(u8::try_from(character).ok()?),

                    0x80..=0x7ff => output.extend_from_slice(&[
                        0xc0 | u8::try_from(character >> 6).ok()?,
                        0x80 | u8::try_from(character & 0x3f).ok()?,
                    ]),

                    0x800..=0xffff => output.extend_from_slice(&[
                        0xe0 | u8::try_from(character >> 12).ok()?,
                        0x80 | u8::try_from((character >> 6) & 0x3f).ok()?,
                        0x80 | u8::try_from(character & 0x3f).ok()?,
                    ]),

                    0x0001_0000..=0x0010_ffff => output.extend_from_slice(&[
                        0xf0 | u8::try_from(character >> 18).ok()?,
                        0x80 | u8::try_from((character >> 12) & 0x3f).ok()?,
                        0x80 | u8::try_from((character >> 6) & 0x3f).ok()?,
                        0x80 | u8::try_from(character & 0x3f).ok()?,
                    ]),

                    _ => return None,
                }

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

            _ => output.push(escape),
        }
    }

    Some(output)
}

fn long(bytes: &[u8]) -> Option<Vec<u8>> {
    let opening = 2 + bytes.get(1..)?.iter().position(|byte| *byte != b'=')?;
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
    } else if content.starts_with(b"\n") {
        content = &content[1..];
    }

    let text = str::from_utf8(content).ok()?;

    Some(text.replace("\r\n", "\n").into_bytes())
}
