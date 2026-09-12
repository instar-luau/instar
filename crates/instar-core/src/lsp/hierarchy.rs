use super::{EditorEntry, Response, Result, failure, path, protocol, state::State};
use std::collections::BTreeMap;

fn item(entry: &EditorEntry) -> Option<protocol::CallHierarchyItem> {
    if !matches!(entry.native.kind, Some(12 | 6)) {
        return None;
    }

    let location = entry.location.as_ref()?;

    Some(protocol::CallHierarchyItem {
        name: entry.native.name.clone()?,
        kind: super::symbol_kind(entry.native.kind),
        tags: None,
        detail: None,
        uri: location.uri.clone(),
        range: location.range,
        selection_range: entry.selection.unwrap_or(location.range),
        data: None,
    })
}

fn target(state: &State, location: &protocol::Location) -> Option<protocol::CallHierarchyItem> {
    state
        .index
        .files
        .get(&path(&location.uri).ok()?)?
        .symbols
        .iter()
        .filter_map(item)
        .find(|item| {
            item.uri == location.uri
                && (item.selection_range == location.range
                    || item.range == location.range
                    || (item.range.start <= location.range.start
                        && item.range.end == location.range.end))
        })
}

pub(super) fn prepare(
    state: &mut State,
    parameters: &protocol::TextDocumentPositionParams,
) -> Result<Vec<protocol::CallHierarchyItem>> {
    state.indexed()?;

    let Response::Editor(entries) = state.query(parameters, "definition")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    Ok(entries
        .iter()
        .filter_map(|entry| {
            entry
                .location
                .as_ref()
                .and_then(|location| target(state, location))
        })
        .collect())
}

fn same(left: &protocol::CallHierarchyItem, right: &protocol::CallHierarchyItem) -> bool {
    left.uri == right.uri
        && left.selection_range == right.selection_range
        && left.name == right.name
}

pub(super) fn calls(
    state: &mut State,
    requested: &protocol::CallHierarchyItem,
    incoming: bool,
) -> Result<Response> {
    path(&requested.uri)?;
    state.indexed()?;
    let mut grouped = BTreeMap::new();

    for (path, file) in &state.index.files {
        let source = state.sources.read(path).map_err(failure)?;
        let uri = protocol::Uri::from_file_path(path).ok_or_else(|| failure("invalid call URI"))?;

        for call in &file.calls {
            let Some(callee) = call
                .location
                .as_ref()
                .and_then(|location| target(state, location))
            else {
                continue;
            };

            let Some(coordinates) = call.native.caller else {
                continue;
            };

            let range = State::source_range(&source, coordinates)?;

            let owner = if let Some(container) = call.native.container {
                let container = State::source_range(&source, container)?;

                file.symbols
                    .iter()
                    .filter_map(item)
                    .find(|item| {
                        item.range.start <= container.start && item.range.end == container.end
                    })
                    .unwrap_or(protocol::CallHierarchyItem {
                        name: "(anonymous)".into(),
                        kind: protocol::SymbolKind::FUNCTION,
                        tags: None,
                        detail: None,
                        uri: uri.clone(),
                        range: container,
                        selection_range: protocol::Range::new(container.start, container.start),
                        data: None,
                    })
            } else {
                let end = source
                    .position(
                        line_index::TextSize::try_from(source.bytes().len()).map_err(failure)?,
                        crate::source::PositionEncoding::Utf16,
                    )
                    .map_err(failure)?;

                protocol::CallHierarchyItem {
                    name: path
                        .file_name()
                        .map_or_else(String::new, |name| name.to_string_lossy().into_owned()),
                    kind: protocol::SymbolKind::MODULE,
                    tags: None,
                    detail: None,
                    uri: uri.clone(),
                    range: protocol::Range::new(
                        protocol::Position::default(),
                        protocol::Position::new(end.line, end.col),
                    ),
                    selection_range: protocol::Range::default(),
                    data: None,
                }
            };

            if !same(if incoming { &callee } else { &owner }, requested) {
                continue;
            }

            let other = if incoming { owner } else { callee };

            let key = (
                other.uri.to_string(),
                other.selection_range.start,
                other.selection_range.end,
            );

            let (_, ranges) = grouped.entry(key).or_insert_with(|| (other, Vec::new()));

            if !ranges.contains(&range) {
                ranges.push(range);
            }
        }
    }

    if incoming {
        Ok(Response::Incoming(
            grouped
                .into_values()
                .map(|(from, from_ranges)| protocol::CallHierarchyIncomingCall {
                    from,
                    from_ranges,
                })
                .collect(),
        ))
    } else {
        Ok(Response::Outgoing(
            grouped
                .into_values()
                .map(|(to, from_ranges)| protocol::CallHierarchyOutgoingCall { to, from_ranges })
                .collect(),
        ))
    }
}
