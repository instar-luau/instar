use instar_project::{EditRequest, SaveRequest};

use tower_lsp_server::{
    gen_lsp_types::{
        DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, TextEdit as ProtocolTextEdit, WillSaveTextDocumentParams,
    },
    jsonrpc::{Error, Result},
};

use super::editing::edit;
use crate::Server;

pub(crate) async fn open(server: &Server, parameters: DidOpenTextDocumentParams) {
    let result = server
        .state
        .lock()
        .expect("session state")
        .open(&server.project, parameters);

    server.schedule_diagnostics();
    server.report(result).await;
}

pub(crate) async fn change(server: &Server, parameters: DidChangeTextDocumentParams) {
    let result = server
        .state
        .lock()
        .expect("session state")
        .change(&server.project, parameters);

    server.schedule_diagnostics();
    server.report(result).await;
}

pub(crate) async fn close(server: &Server, parameters: DidCloseTextDocumentParams) {
    let result = server
        .state
        .lock()
        .expect("session state")
        .close(&server.project, parameters);

    server.schedule_diagnostics();
    server.report(result).await;
}

pub(crate) async fn save(server: &Server, parameters: DidSaveTextDocumentParams) {
    let result = server
        .state
        .lock()
        .expect("session state")
        .save(&server.project, parameters);

    server.schedule_diagnostics();
    server.report(result).await;
}

pub(crate) async fn will_save(server: &Server, parameters: WillSaveTextDocumentParams) {
    let result = server.capture(&parameters.text_document.uri).map(|_| ());
    server.report(result).await;
}

pub(crate) async fn save_edits(
    server: &Server,
    parameters: WillSaveTextDocumentParams,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    server
        .state
        .lock()
        .expect("session state")
        .editable(&parameters.text_document.uri)?;

    let (snapshot, source) = server.capture(&parameters.text_document.uri)?;

    let report = edit(
        server,
        EditRequest::Save(SaveRequest {
            snapshot,
            source: source.version,
        }),
        None,
    )
    .await?;

    let Some(validated) = report.validated else {
        if report.proposal.is_some() {
            return Err(Error::invalid_params(
                "save edits did not complete validation",
            ));
        }

        return Ok(None);
    };

    let edits = validated
        .proposal()
        .edits
        .iter()
        .map(|edit| {
            if edit.location.source != source.version {
                return Err(Error::invalid_params(
                    "will-save edits must stay in the saving document",
                ));
            }

            server.text_edit(&report.snapshot, edit)
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(Some(edits))
}
