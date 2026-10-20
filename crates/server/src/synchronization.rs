use std::{path::Path, sync::Arc};

use tower_lsp_server::{
    gen_lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams,
    },
    jsonrpc::{Error, Result},
};

use super::{
    document::{self, Document},
    project::{Project, failure},
};

pub(super) fn open(project: &mut Project, parameters: DidOpenTextDocumentParams) -> Result<()> {
    let document = parameters.text_document;
    let path = document::path(&document.uri)?;
    let source: Arc<str> = Arc::from(document.text);

    let module = project
        .workspace
        .set_source(&path, Arc::clone(&source))
        .map_err(failure)?;

    let revision = project.advance()?;

    project.documents.insert(
        module,
        Document::new(document.uri, source, Some(document.version), revision),
    );

    project.changed()
}

pub(super) fn change(project: &mut Project, parameters: DidChangeTextDocumentParams) -> Result<()> {
    let module = project.module(&parameters.text_document.text_document_identifier.uri)?;

    let previous = project
        .documents
        .get(&module)
        .cloned()
        .ok_or_else(|| Error::invalid_params("Document is not open."))?;

    if previous.version.is_none() {
        return Err(Error::invalid_params("Document is not open."));
    }

    let revision = project.advance()?;

    let document = previous.changed(
        parameters.text_document.version,
        parameters.content_changes,
        project.encoding,
        revision,
    )?;

    project
        .workspace
        .set_source(Path::new(&module), Arc::clone(&document.text))
        .map_err(failure)?;

    project.documents.insert(module, document);

    project.changed()
}

pub(super) fn save(project: &mut Project, parameters: DidSaveTextDocumentParams) -> Result<()> {
    let module = project.module(&parameters.text_document.uri)?;
    let previous = project.snapshot(&module)?;

    let Some(text) = parameters
        .text
        .filter(|text| text.as_str() != previous.text.as_ref())
    else {
        return Ok(());
    };

    let revision = project.advance()?;
    let source: Arc<str> = Arc::from(text);

    project
        .workspace
        .set_source(Path::new(&module), Arc::clone(&source))
        .map_err(failure)?;

    project.documents.insert(
        module,
        Document::new(previous.uri, source, previous.version, revision),
    );

    project.changed()
}

pub(super) fn close(project: &mut Project, parameters: &DidCloseTextDocumentParams) -> Result<()> {
    let module = project.module(&parameters.text_document.uri)?;

    if project
        .documents
        .get(&module)
        .is_none_or(|document| document.version.is_none())
    {
        return Ok(());
    }

    project.workspace.remove_source(&module);
    project.documents.remove(&module);
    project.tokens.remove(&module);

    project.changed()
}
