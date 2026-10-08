use tower_lsp_server::{
    gen_lsp_types::{Position, PositionEncodingKind, Range},
    jsonrpc::{Error, Result},
};

pub(crate) fn offset(
    text: &str,
    position: Position,
    encoding: &PositionEncodingKind,
) -> Result<usize> {
    let mut line = 0;
    let mut start = 0;
    let mut characters = text.char_indices().peekable();

    while line < position.line {
        let Some((index, character)) = characters.next() else {
            return Err(Error::invalid_params("position line exceeds document"));
        };

        if character == '\r' || character == '\n' {
            start = index + character.len_utf8();

            if character == '\r'
                && characters
                    .peek()
                    .is_some_and(|(_, character)| *character == '\n')
            {
                characters.next();
                start += 1;
            }

            line += 1;
        }
    }

    let mut units = 0;

    for (index, character) in text[start..].char_indices() {
        if units == position.character || character == '\r' || character == '\n' {
            return Ok(start + index);
        }

        units += width(character, encoding);

        if units > position.character {
            return Err(Error::invalid_params("position splits a Unicode character"));
        }
    }

    Ok(text.len())
}

pub(crate) fn end(text: &str, encoding: &PositionEncodingKind) -> Position {
    let mut position = Position::default();
    let mut characters = text.chars().peekable();

    while let Some(character) = characters.next() {
        if character == '\r' || character == '\n' {
            if character == '\r' && characters.peek() == Some(&'\n') {
                characters.next();
            }

            position.line += 1;
            position.character = 0;
        } else {
            position.character += width(character, encoding);
        }
    }

    position
}

pub(crate) fn replace(
    text: &mut String,
    range: Range,
    replacement: &str,
    encoding: &PositionEncodingKind,
) -> Result<()> {
    let start = offset(text, range.start, encoding)?;
    let end = offset(text, range.end, encoding)?;

    if start > end {
        return Err(Error::invalid_params("edit range is reversed"));
    }

    text.replace_range(start..end, replacement);

    Ok(())
}

fn width(character: char, encoding: &PositionEncodingKind) -> u32 {
    match encoding {
        PositionEncodingKind::UTF8 => {
            u32::try_from(character.len_utf8()).expect("Unicode byte width")
        }

        PositionEncodingKind::UTF32 => 1,
        _ => u32::try_from(character.len_utf16()).expect("Unicode unit width"),
    }
}
