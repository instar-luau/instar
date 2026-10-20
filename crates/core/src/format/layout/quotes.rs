use crate::settings::format::Quotes;

pub(super) fn quote(bytes: &[u8], style: Quotes) -> Option<Vec<u8>> {
    if style == Quotes::Preserve {
        return None;
    }

    let mut content = Vec::new();
    let mut position = 1;

    while position + 1 < bytes.len() {
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

    let has_single_quote = content.contains(&b'\'');
    let has_double_quote = content.contains(&b'"');

    let delimiter = match style {
        Quotes::Single => b'\'',
        Quotes::Double | Quotes::Preserve => b'"',

        Quotes::PreferSingle => {
            if has_single_quote && !has_double_quote {
                b'"'
            } else {
                b'\''
            }
        }

        Quotes::PreferDouble => {
            if has_double_quote && !has_single_quote {
                b'\''
            } else {
                b'"'
            }
        }
    };

    let mut result = vec![delimiter];
    let mut position = 0;

    while position < content.len() {
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

    (result.as_slice() != bytes).then_some(result)
}
