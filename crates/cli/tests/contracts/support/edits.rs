use serde_json::Value;

pub(crate) fn apply(source: &str, edits: &Value, encoding: &str) -> String {
    let mut replacements = edits
        .as_array()
        .expect("LSP text edits")
        .iter()
        .map(|edit| {
            let start = offset(source, &edit["range"]["start"], encoding);
            let end = offset(source, &edit["range"]["end"], encoding);
            assert!(start <= end, "LSP edit range is ordered");
            (
                start,
                end,
                edit["newText"].as_str().expect("LSP replacement text"),
            )
        })
        .collect::<Vec<_>>();
    replacements.sort_by_key(|replacement| (replacement.0, replacement.1));
    for pair in replacements.windows(2) {
        assert!(pair[0].1 <= pair[1].0, "LSP edits must not overlap");
    }
    let mut output = source.to_owned();
    for (start, end, replacement) in replacements.into_iter().rev() {
        output.replace_range(start..end, replacement);
    }
    output
}

fn offset(source: &str, position: &Value, encoding: &str) -> usize {
    let line = usize::try_from(position["line"].as_u64().expect("nonnegative LSP line"))
        .expect("LSP line fits source indexing");
    let character = usize::try_from(
        position["character"]
            .as_u64()
            .expect("nonnegative LSP character"),
    )
    .expect("LSP character fits source indexing");
    let mut lines = Vec::new();
    let mut start = 0;
    let mut characters = source.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        if character == '\r' || character == '\n' {
            lines.push((start, index));
            start = index + 1;
            if character == '\r'
                && characters
                    .peek()
                    .is_some_and(|(_, following)| *following == '\n')
            {
                let (index, _) = characters.next().expect("CRLF continuation");
                start = index + 1;
            }
        }
    }
    lines.push((start, source.len()));
    let (start, end) = *lines.get(line).expect("LSP line is within the source");
    let text = &source[start..end];
    let mut units = 0;
    for (index, current) in text.char_indices() {
        if units == character {
            return start + index;
        }
        units += match encoding {
            "utf-8" => current.len_utf8(),
            "utf-16" => current.len_utf16(),
            "utf-32" => 1,
            _ => panic!("unsupported LSP position encoding: {encoding}"),
        };
    }
    assert_eq!(
        units, character,
        "LSP character is a source code-point boundary"
    );
    end
}
