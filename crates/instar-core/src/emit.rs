use std::{io, ops::Range};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

pub(crate) fn apply(source: &str, mut edits: Vec<Edit>) -> io::Result<String> {
    edits.sort_by_key(|edit| (edit.range.start, edit.range.end));

    let mut output = String::with_capacity(source.len());
    let mut previous = 0;
    let mut last_start = None;

    for edit in edits {
        if last_start == Some(edit.range.start)
            || edit.range.start < previous
            || edit.range.end < edit.range.start
            || source.get(edit.range.clone()).is_none()
        {
            return Err(io::Error::other("overlapping or invalid edits"));
        }

        output.push_str(&source[previous..edit.range.start]);
        output.push_str(&edit.text);
        last_start = Some(edit.range.start);
        previous = edit.range.end;
    }

    output.push_str(&source[previous..]);

    Ok(output)
}
