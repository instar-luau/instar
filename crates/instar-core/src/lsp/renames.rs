use super::{Response, Result, failure, imports, moved, path, protocol, state::State};

fn relocated(uri: &protocol::Uri, files: &[protocol::FileRename]) -> Result<protocol::Uri> {
    for file in files {
        let old = path(&file.old_uri.parse().map_err(failure)?)?;
        let new = file.new_uri.parse().map_err(failure)?;

        if let Some(uri) = moved(uri, &old, &new)? {
            return Ok(uri);
        }
    }

    Ok(uri.clone())
}

pub(super) fn edits(
    state: &mut State,
    parameters: &protocol::RenameFilesParams,
) -> Result<Response> {
    state.indexed()?;
    let modules = state.index.files.keys().cloned().collect::<Vec<_>>();
    let mut documents = Vec::new();

    for module in modules {
        let uri =
            protocol::Uri::from_file_path(&module).ok_or_else(|| failure("invalid module URI"))?;

        let destination = relocated(&uri, &parameters.files)?;
        let source = state.sources.read(&module).map_err(failure)?;

        let Response::Editor(links) = state.query(
            &protocol::TextDocumentPositionParams {
                text_document: protocol::TextDocumentIdentifier { uri: uri.clone() },
                position: protocol::Position::default(),
            },
            "links",
        )?
        else {
            return Err(tower_lsp_server::jsonrpc::Error::internal_error());
        };

        let mut edits = Vec::new();

        for link in links {
            let Some(target) = link
                .native
                .name
                .and_then(|name| protocol::Uri::from_file_path(std::path::Path::new(&name)))
            else {
                continue;
            };

            let updated = relocated(&target, &parameters.files)?;

            if target == updated && uri == destination {
                continue;
            }

            let Some(location) = link.location else {
                continue;
            };

            let offsets = super::formatting::offsets(&source, location.range)?;
            let literal = &source.text().map_err(failure)?[offsets];

            let Some(quote) = literal
                .chars()
                .next()
                .filter(|quote| matches!(quote, '\'' | '"'))
            else {
                continue;
            };

            if !literal.ends_with(quote) {
                continue;
            }

            let Some(specifier) = imports::specifier(&path(&destination)?, &path(&updated)?) else {
                continue;
            };

            let escaped = specifier
                .chars()
                .map(|character| {
                    if character == quote || character == '\\' || character.is_control() {
                        character.escape_default().to_string()
                    } else {
                        character.to_string()
                    }
                })
                .collect::<String>();

            edits.push(protocol::OneOf::Left(protocol::TextEdit {
                range: location.range,
                new_text: format!("{quote}{escaped}{quote}"),
            }));
        }

        if !edits.is_empty() {
            documents.push(protocol::TextDocumentEdit {
                text_document: protocol::OptionalVersionedTextDocumentIdentifier {
                    uri,
                    version: state.version(&module),
                },
                edits,
            });
        }
    }

    Ok(Response::Rename(protocol::WorkspaceEdit {
        document_changes: Some(protocol::DocumentChanges::Edits(documents)),
        ..protocol::WorkspaceEdit::default()
    }))
}
