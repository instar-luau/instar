use super::{Backend, Position, Result, protocol};
use std::collections::BTreeMap;

pub(super) async fn highlights(
    server: &Backend,
    parameters: protocol::DocumentHighlightParams,
) -> Result<Option<Vec<protocol::DocumentHighlight>>> {
    let identifier = parameters
        .text_document_position_params
        .text_document
        .uri
        .clone();

    let entries = server
        .query(parameters.text_document_position_params, "localReferences")
        .await?;

    Ok(Some(
        entries
            .into_iter()
            .filter_map(|entry| {
                let location = entry.location?;

                (location.uri == identifier).then_some((
                    (location.range.start, location.range.end),
                    protocol::DocumentHighlight {
                        range: location.range,
                        kind: None,
                    },
                ))
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect(),
    ))
}

pub(super) async fn folds(
    server: &Backend,
    parameters: protocol::FoldingRangeParams,
) -> Result<Option<Vec<protocol::FoldingRange>>> {
    let entries = server
        .query(
            protocol::TextDocumentPositionParams {
                text_document: parameters.text_document,
                position: Position::default(),
            },
            "folds",
        )
        .await?;

    Ok(Some(
        entries
            .into_iter()
            .filter_map(|entry| {
                let range = entry.location?.range;

                (range.start.line < range.end.line).then_some((
                    (range.start, range.end),
                    protocol::FoldingRange {
                        start_line: range.start.line,
                        start_character: Some(range.start.character),
                        end_line: range.end.line,
                        end_character: Some(range.end.character),
                        ..protocol::FoldingRange::default()
                    },
                ))
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect(),
    ))
}

pub(super) async fn selections(
    server: &Backend,
    parameters: protocol::SelectionRangeParams,
) -> Result<Option<Vec<protocol::SelectionRange>>> {
    let mut ranges = Vec::new();

    for position in parameters.positions {
        let entries = server
            .query(
                protocol::TextDocumentPositionParams {
                    text_document: parameters.text_document.clone(),
                    position,
                },
                "selection",
            )
            .await?;

        let mut parent: Option<Box<protocol::SelectionRange>> = None;

        for entry in entries {
            let Some(location) = entry.location else {
                continue;
            };

            let range = location.range;

            if range.start > position
                || range.end < position
                || parent.as_ref().is_some_and(|parent| {
                    range == parent.range
                        || range.start < parent.range.start
                        || range.end > parent.range.end
                })
            {
                continue;
            }

            parent = Some(Box::new(protocol::SelectionRange { range, parent }));
        }

        ranges.push(parent.map_or_else(
            || protocol::SelectionRange {
                range: protocol::Range::new(position, position),
                parent: None,
            },
            |range| *range,
        ));
    }

    Ok(Some(ranges))
}
