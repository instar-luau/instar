use super::{Result, failure, protocol};
use crate::source::{PositionEncoding, Source};
use line_index::LineCol;
use tower_lsp_server::jsonrpc::Error;

pub(super) fn apply(
    source: &Source,
    changes: Vec<protocol::TextDocumentContentChangeEvent>,
) -> Result<String> {
    let mut text = source.text().map_err(failure)?.to_owned();

    for change in changes {
        if let Some(range) = change.range {
            let snapshot =
                Source::new(source.path().to_owned(), text.as_bytes().to_vec()).map_err(failure)?;

            let offset = |position: protocol::Position| {
                snapshot
                    .offset(
                        LineCol {
                            line: position.line,
                            col: position.character,
                        },
                        PositionEncoding::Utf16,
                    )
                    .map(usize::from)
                    .map_err(failure)
            };

            let start = offset(range.start)?;
            let end = offset(range.end)?;

            if start > end {
                return Err(Error::invalid_params("reversed change range"));
            }

            text.replace_range(start..end, &change.text);
        } else {
            text = change.text;
        }
    }

    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn malformed_changes_preserve_the_snapshot() -> Result<()> {
        let source = Source::new("main.luau".into(), "😀\r\nreturn 1".as_bytes().to_vec())
            .map_err(failure)?;

        for (start, end) in [(1, 2), (2, 0), (0, 99)] {
            let change = protocol::TextDocumentContentChangeEvent {
                range: Some(protocol::Range::new(
                    protocol::Position::new(0, start),
                    protocol::Position::new(0, end),
                )),
                range_length: None,
                text: "replacement".into(),
            };

            assert!(apply(&source, vec![change]).is_err());
            assert_eq!(source.text().map_err(failure)?, "😀\r\nreturn 1");
        }

        Ok(())
    }
}
