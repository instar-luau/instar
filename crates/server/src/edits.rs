use std::collections::BTreeMap;

use instar_project::{ProjectSnapshot, ValidatedEdit};
use instar_shared::TextEdit;

use tower_lsp_server::{
    gen_lsp_types::{
        OptionalVersionedTextDocumentIdentifier, TextDocumentEdit, TextDocumentIdentifier,
        TextEdit as ProtocolTextEdit, Uri, WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use crate::{Server, position, session::failure};

impl Server {
    pub(crate) fn workspace_edit(
        &self,
        snapshot: &ProjectSnapshot,
        validated: &ValidatedEdit,
        require_synchronized: bool,
    ) -> Result<WorkspaceEdit> {
        let state = self.state.lock().expect("session state");

        if !state.current(&self.project, validated.inputs()) {
            return Err(Error::content_modified());
        }

        let mut edits = validated.proposal().edits.clone();
        edits.sort_by_key(|edit| (edit.location.source, edit.location.span));

        if edits.windows(2).any(|pair| {
            pair[0].location.source == pair[1].location.source
                && pair[0].location.span.end > pair[1].location.span.start
        }) {
            return Err(Error::invalid_params("workspace edits overlap"));
        }

        let mut grouped = BTreeMap::<Uri, Vec<ProtocolTextEdit>>::new();
        let mut versions = BTreeMap::new();

        for edit in edits {
            let source = snapshot
                .source(edit.location.source.identity)
                .ok_or_else(Error::content_modified)?;

            edit.location
                .validate(source)
                .map_err(|error| failure(&error))?;

            let current = self.project.snapshot();

            if current
                .source(source.version.identity)
                .is_none_or(|current| current.version != source.version)
            {
                return Err(Error::content_modified());
            }

            let uri = state.uri(snapshot, source.version.identity)?;
            state.editable(&uri)?;
            let document = state.documents.get(&uri);

            if require_synchronized && document.is_none() {
                return Err(Error::invalid_params(
                    "standard workspace/applyEdit cannot protect closed files from external changes; open the affected files or use the protected project writer",
                ));
            }

            versions.insert(uri.clone(), document.map(|document| document.version));

            grouped.entry(uri).or_default().push(ProtocolTextEdit {
                range: position::range(
                    &source.text,
                    [edit.location.span.start, edit.location.span.end],
                    &state.encoding,
                )
                .map_err(|error| failure(&error))?,
                new_text: edit.replacement,
            });
        }

        if state
            .capabilities
            .supports("/workspace/workspaceEdit/documentChanges")
        {
            Ok(WorkspaceEdit {
                document_changes: Some(
                    grouped
                        .into_iter()
                        .map(|(uri, edits)| {
                            TextDocumentEdit {
                                text_document: OptionalVersionedTextDocumentIdentifier {
                                    text_document_identifier: TextDocumentIdentifier {
                                        uri: uri.clone(),
                                    },
                                    version: versions[&uri],
                                },
                                edits: edits.into_iter().map(Into::into).collect(),
                            }
                            .into()
                        })
                        .collect(),
                ),
                ..WorkspaceEdit::default()
            })
        } else {
            if require_synchronized && !grouped.is_empty() {
                return Err(Error::invalid_params(
                    "client must support versioned document edits for guarded application",
                ));
            }

            Ok(WorkspaceEdit {
                changes: Some(grouped.into_iter().collect()),
                ..WorkspaceEdit::default()
            })
        }
    }

    pub(crate) fn text_edit(
        &self,
        snapshot: &ProjectSnapshot,
        edit: &TextEdit,
    ) -> Result<ProtocolTextEdit> {
        Ok(ProtocolTextEdit {
            range: self.range(snapshot, edit.location)?,
            new_text: edit.replacement.clone(),
        })
    }
}
