use super::{Response, Result, failure, path, protocol, state::State};
use crate::source::PositionEncoding;
use line_index::LineCol;
use std::collections::{BTreeSet, HashMap};

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

    if wants(&parameters, "quickfix") {
        actions.extend(fixes(state, &parameters)?);
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

fn fixes(
    state: &mut State,
    parameters: &protocol::CodeActionParams,
) -> Result<protocol::CodeActionResponse> {
    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(failure)?;

    let tokens = state.query(
        &protocol::TextDocumentPositionParams {
            text_document: parameters.text_document.clone(),
            position: protocol::Position::default(),
        },
        "tokens",
    )?;

    let names = if let Response::Editor(entries) = tokens {
        entries
            .into_iter()
            .filter_map(|entry| entry.native.name)
            .collect::<BTreeSet<_>>()
    } else {
        BTreeSet::new()
    };

    let mut actions = Vec::new();

    for diagnostic in &parameters.context.diagnostics {
        if diagnostic.source.as_deref() != Some("instar")
            || !matches!(diagnostic.code.as_ref(), Some(protocol::NumberOrString::String(code)) if code == "LocalUnused" || code == "FunctionUnused")
        {
            continue;
        }

        let offset = |position: protocol::Position| {
            source
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

        let start = offset(diagnostic.range.start)?;
        let end = offset(diagnostic.range.end)?;

        let Some(name) = source.text().map_err(failure)?.get(start..end) else {
            continue;
        };

        if name.is_empty()
            || name.starts_with('_')
            || !name
                .bytes()
                .all(|character| character.is_ascii_alphanumeric() || character == b'_')
            || name.as_bytes()[0].is_ascii_digit()
        {
            continue;
        }

        let replacement = format!("_{name}");

        if names.contains(&replacement) {
            continue;
        }

        actions.push(protocol::CodeActionOrCommand::CodeAction(
            protocol::CodeAction {
                title: format!("Prefix '{name}' with '_'"),
                kind: Some(protocol::CodeActionKind::QUICKFIX),
                diagnostics: Some(vec![diagnostic.clone()]),
                edit: Some(protocol::WorkspaceEdit {
                    document_changes: Some(protocol::DocumentChanges::Edits(vec![
                        protocol::TextDocumentEdit {
                            text_document: protocol::OptionalVersionedTextDocumentIdentifier {
                                uri: parameters.text_document.uri.clone(),
                                version: state.version(source.path()),
                            },
                            edits: vec![protocol::OneOf::Left(protocol::TextEdit {
                                range: diagnostic.range,
                                new_text: replacement,
                            })],
                        },
                    ])),
                    ..protocol::WorkspaceEdit::default()
                }),
                ..protocol::CodeAction::default()
            },
        ));
    }

    Ok(actions)
}
