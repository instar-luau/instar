use super::{Response, Result, protocol, state::State};
use std::collections::HashMap;

fn wants(parameters: &protocol::CodeActionParams, kind: &str) -> bool {
    parameters.context.only.as_ref().is_none_or(|kinds| {
        kinds.iter().any(|requested| {
            kind == requested.as_str()
                || kind
                    .strip_prefix(requested.as_str())
                    .is_some_and(|suffix| suffix.starts_with('.'))
        })
    })
}

pub(super) fn actions(
    state: &mut State,
    parameters: protocol::CodeActionParams,
) -> Result<protocol::CodeActionResponse> {
    let mut actions = Vec::new();

    if wants(&parameters, "refactor.extract") {
        actions.extend(super::refactor::extract(state, &parameters)?);
    }

    let individual = wants(&parameters, "quickfix");
    let all = wants(&parameters, "source.fixAll");

    if individual || all {
        actions.extend(super::lint::actions(state, &parameters, individual, all)?);
    }

    if individual {
        actions.extend(super::imports::fixes(state, &parameters)?);
    }

    if wants(&parameters, "source.format")
        && let Ok(Response::Edits(Some(edits))) = state.format(&parameters.text_document.uri)
    {
        actions.push(protocol::CodeActionOrCommand::CodeAction(
            protocol::CodeAction {
                title: "Format document".into(),
                kind: Some(protocol::CodeActionKind::new("source.format")),
                edit: Some(protocol::WorkspaceEdit {
                    changes: Some(HashMap::from([(parameters.text_document.uri, edits)])),
                    ..protocol::WorkspaceEdit::default()
                }),
                ..protocol::CodeAction::default()
            },
        ));
    }

    Ok(actions)
}
