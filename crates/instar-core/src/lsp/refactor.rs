use super::{Response, Result, failure, path, protocol, state::State};
use std::collections::BTreeSet;

pub(super) fn extract(
    state: &mut State,
    parameters: &protocol::CodeActionParams,
) -> Result<Vec<protocol::CodeActionOrCommand>> {
    if parameters.range.start == parameters.range.end {
        return Ok(Vec::new());
    }

    let position = protocol::TextDocumentPositionParams {
        text_document: parameters.text_document.clone(),
        position: parameters.range.start,
    };

    let Response::Editor(entries) = state.query(&position, "extract")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    let Some(entry) = entries.into_iter().find(|entry| {
        entry
            .location
            .as_ref()
            .is_some_and(|location| location.range == parameters.range)
    }) else {
        return Ok(Vec::new());
    };

    let Some(statement) = entry.selection else {
        return Ok(Vec::new());
    };

    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(failure)?;

    let offsets = super::formatting::offsets(&source, parameters.range)?;
    let text = source.text().map_err(failure)?;

    let Response::Editor(tokens) = state.query(&position, "tokens")? else {
        return Err(tower_lsp_server::jsonrpc::Error::internal_error());
    };

    let names = tokens
        .into_iter()
        .filter_map(|entry| entry.native.name)
        .collect::<BTreeSet<_>>();

    let mut name = "value".to_owned();
    let mut suffix = 1;

    while names.contains(&name) {
        suffix += 1;
        name = format!("value{suffix}");
    }

    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };

    let insertion = statement.start;

    let edits = vec![
        protocol::OneOf::Left(protocol::TextEdit {
            range: parameters.range,
            new_text: name.clone(),
        }),
        protocol::OneOf::Left(protocol::TextEdit {
            range: protocol::Range::new(insertion, insertion),
            new_text: format!("local {name} = {}{newline}", &text[offsets]),
        }),
    ];

    Ok(vec![protocol::CodeActionOrCommand::CodeAction(
        protocol::CodeAction {
            title: "Extract local variable".into(),
            kind: Some(protocol::CodeActionKind::REFACTOR_EXTRACT),
            edit: Some(protocol::WorkspaceEdit {
                document_changes: Some(protocol::DocumentChanges::Edits(vec![
                    protocol::TextDocumentEdit {
                        text_document: protocol::OptionalVersionedTextDocumentIdentifier {
                            uri: parameters.text_document.uri.clone(),
                            version: state.version(source.path()),
                        },
                        edits,
                    },
                ])),
                ..protocol::WorkspaceEdit::default()
            }),
            ..protocol::CodeAction::default()
        },
    )])
}
