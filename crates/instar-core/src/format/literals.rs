use std::borrow::Cow;

use super::{Quotes, Zero};

pub(super) fn quote(text: &str, style: Quotes) -> Cow<'_, str> {
    if !text.starts_with(['\'', '"']) || style == Quotes::Preserve {
        return Cow::Borrowed(text);
    }

    let inner = &text[1..text.len() - 1];
    let mut doubles = 0;
    let mut singles = 0;
    let mut escaped = false;

    for character in inner.chars() {
        match character {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => doubles += 1,
            '\'' => singles += 1,
            _ => {}
        }
    }

    let target = match style {
        Quotes::ForceSingle => '\'',
        Quotes::AutoPreferDouble if doubles > singles => '\'',
        Quotes::AutoPreferSingle if singles <= doubles => '\'',
        _ => '"',
    };

    if text.starts_with(target) {
        return Cow::Borrowed(text);
    }

    let other = if target == '"' { '\'' } else { '"' };

    let mut output = String::new();
    output.push(target);
    let mut characters = inner.chars();

    while let Some(character) = characters.next() {
        match character {
            '\\' => match characters.next() {
                Some(next) if next == other => output.push(other),

                Some(next) => {
                    output.push('\\');
                    output.push(next);
                }

                None => output.push('\\'),
            },

            character if character == target => {
                output.push('\\');
                output.push(target);
            }

            character => output.push(character),
        }
    }

    output.push(target);

    Cow::Owned(output)
}

pub(super) fn number(text: &str, style: Zero) -> Cow<'_, str> {
    match style {
        Zero::Add
            if text.starts_with('.') && text.as_bytes().get(1).is_some_and(u8::is_ascii_digit) =>
        {
            Cow::Owned(format!("0{text}"))
        }

        Zero::Strip
            if text.starts_with("0.") && text.as_bytes().get(2).is_some_and(u8::is_ascii_digit) =>
        {
            Cow::Borrowed(&text[1..])
        }

        _ => Cow::Borrowed(text),
    }
}
