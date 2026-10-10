use instar_shared::{Options, Reason};

use crate::settings::QuoteStyle;

pub(super) fn quote(
    bytes: &[u8],
    style: QuoteStyle,
    options: &Options,
) -> Result<Option<Vec<u8>>, Reason> {
    if style == QuoteStyle::Preserve {
        return Ok(None);
    }

    let mut content = Vec::new();
    let mut position = 1;

    while position + 1 < bytes.len() {
        if let Some(reason) = options.interrupted().map(|_| Reason::FailedPrerequisite) {
            return Err(reason);
        }

        let byte = bytes[position];

        if byte == b'\\' && position + 2 < bytes.len() {
            let next = bytes[position + 1];

            if matches!(next, b'\'' | b'"') {
                content.push(next);
            } else {
                content.extend_from_slice(&bytes[position..position + 2]);
            }

            position += 2;
        } else {
            content.push(byte);
            position += 1;
        }
    }

    let single = content.contains(&b'\'');
    let double = content.contains(&b'"');

    let delimiter = match style {
        QuoteStyle::Single => b'\'',
        QuoteStyle::Double | QuoteStyle::Preserve => b'"',

        QuoteStyle::PreferSingle => {
            if single && !double {
                b'"'
            } else {
                b'\''
            }
        }

        QuoteStyle::PreferDouble => {
            if double && !single {
                b'\''
            } else {
                b'"'
            }
        }
    };

    let mut result = vec![delimiter];
    let mut position = 0;

    while position < content.len() {
        if let Some(reason) = options.interrupted().map(|_| Reason::FailedPrerequisite) {
            return Err(reason);
        }

        let byte = content[position];

        if byte == b'\\' && position + 1 < content.len() {
            result.extend_from_slice(&content[position..position + 2]);
            position += 2;
        } else {
            if byte == delimiter {
                result.push(b'\\');
            }

            result.push(byte);
            position += 1;
        }
    }

    result.push(delimiter);

    Ok((result.as_slice() != bytes).then_some(result))
}
