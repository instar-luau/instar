use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use serde_json::{Value, json};
use tokio::sync;

use tower_lsp_server::{
    Client, LanguageServer,
    gen_lsp_types::{
        CodeActionParams, CodeActionResponse, ColorInformation, ColorPresentation,
        ColorPresentationParams, CompletionItem, CompletionParams, CompletionResponse,
        CreateFilesParams, DefinitionParams, DefinitionResponse, DeleteFilesParams,
        DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, DocumentColorParams, DocumentFormattingParams,
        DocumentHighlight, DocumentHighlightParams, DocumentLink, DocumentLinkParams,
        DocumentRangeFormattingParams, DocumentSymbolParams, DocumentSymbolResponse,
        ExecuteCommandParams, FoldingRange, FoldingRangeParams, Hover, HoverParams,
        InitializeParams, InitializeResult, InitializedParams, InlayHint, InlayHintParams,
        Location, PrepareRenameResult, ReferenceParams, RenameFilesParams, RenameParams,
        SelectionRange, SelectionRangeParams, SemanticTokens, SemanticTokensDeltaParams,
        SemanticTokensDeltaResponse, SemanticTokensParams, SemanticTokensRangeParams,
        ShowDocumentParams, SignatureHelp, SignatureHelpParams, TextDocumentPositionParams,
        TextEdit, TypeDefinitionParams, TypeDefinitionResponse, Uri, WorkDoneProgressCancelParams,
        WorkspaceEdit, WorkspaceSymbolParams, WorkspaceSymbolResponse,
    },
    jsonrpc::{Error, Result},
};

use super::registration::Registrations;

use crate::{
    configuration::Configuration,
    connection::{Connection, Consistency, Priority, Target},
    document::{self, Encoding},
    editing,
    error::{failure, reason},
    files, navigation,
    output::{messages, progress::Registry},
    presentation,
    project::resolved_uri,
    suggestions, synchronization,
};

pub(crate) struct Service {
    pub(super) client: Client,
    pub(super) connection: Arc<Connection>,
    pub(super) has_shutdown: Arc<AtomicBool>,
    pub(super) capabilities: Mutex<Arc<Value>>,
    pub(super) registrations: sync::Mutex<Registrations>,
    pub(super) formatting: Arc<sync::Semaphore>,
    pub(super) progress: Registry,
    pub(super) sequence: AtomicU64,
    pub(super) configuration_sequence: AtomicU64,
}

impl Service {
    pub(crate) fn new(
        client: Client,
        connection: Arc<Connection>,
        has_shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            connection,
            has_shutdown,
            capabilities: Mutex::new(Arc::new(Value::Null)),
            registrations: sync::Mutex::new(Registrations::default()),
            formatting: Arc::new(sync::Semaphore::new(1)),
            progress: Arc::new(Mutex::new(HashMap::new())),
            sequence: AtomicU64::new(0),
            configuration_sequence: AtomicU64::new(0),
        }
    }

    pub(crate) fn cancel_progress(&self, parameters: &WorkDoneProgressCancelParams) {
        if let Some(operation) = self
            .progress
            .lock()
            .expect("Internal error: progress registry mutex is poisoned.")
            .get(&parameters.token)
            .and_then(Weak::upgrade)
        {
            operation.cancel();
        }
    }

    pub(super) fn has_capability(&self, pointer: &str) -> bool {
        self.capabilities
            .lock()
            .expect("Internal error: client capabilities mutex is poisoned.")
            .pointer(pointer)
            .and_then(Value::as_bool)
            == Some(true)
    }
}

impl LanguageServer for Service {
    async fn initialize(&self, parameters: InitializeParams) -> Result<InitializeResult> {
        let value = serde_json::to_value(&parameters).map_err(failure)?;

        let capabilities =
            Arc::new(serde_json::to_value(parameters.capabilities).map_err(failure)?);

        let configuration =
            Configuration::load(parameters.initialization_options.unwrap_or(Value::Null))?;

        let encoding = capabilities
            .pointer("/general/positionEncodings")
            .and_then(Value::as_array)
            .and_then(|values| {
                values.iter().find_map(|value| match value.as_str() {
                    Some("utf-8") => Some(Encoding::Bytes),
                    Some("utf-16") => Some(Encoding::Units),
                    Some("utf-32") => Some(Encoding::Scalars),
                    _ => None,
                })
            })
            .unwrap_or_default();

        let roots =
            if let Some(folders) = value.get("workspaceFolders").and_then(Value::as_array) {
                folders
                    .iter()
                    .map(|folder| {
                        serde_json::from_value(folder.get("uri").cloned().ok_or_else(|| {
                            Error::invalid_params("Workspace folder URI is missing.")
                        })?)
                        .map_err(failure)
                    })
                    .collect::<Result<Vec<Uri>>>()?
            } else if let Some(uri) = value.get("rootUri").filter(|uri| !uri.is_null()) {
                vec![serde_json::from_value(uri.clone()).map_err(failure)?]
            } else {
                Vec::new()
            };

        *self
            .capabilities
            .lock()
            .expect("Internal error: client capabilities mutex is poisoned.") =
            Arc::clone(&capabilities);

        let client = Arc::clone(&capabilities);

        self.connection
            .control(move |state| {
                state.capabilities = client;
                state.configuration = configuration;
                state.encoding = encoding;

                for root in roots {
                    state.add(&root)?;
                }

                Ok(())
            })
            .await?;

        serde_json::from_value(json!({
            "capabilities": {
                "positionEncoding": encoding.protocol(),
                "textDocumentSync": {"openClose":true,"change":2,"save":{"includeText":true}},
                "hoverProvider":true,
                "completionProvider":{"resolveProvider":true,"triggerCharacters":[".",":","\"","'","/","@"],"workDoneProgress":true},
                "signatureHelpProvider":{"triggerCharacters":["(",","],"retriggerCharacters":[")"],"workDoneProgress":true},
                "codeActionProvider":{"codeActionKinds":["quickfix","source.fixAll.instar","source.organizeImports"],"workDoneProgress":true},
                "inlayHintProvider":{"resolveProvider":true},
                "definitionProvider":true,"typeDefinitionProvider":true,"referencesProvider":true,
                "documentHighlightProvider":true,"documentSymbolProvider":true,
                "workspaceSymbolProvider":{"workDoneProgress":true},
                "documentLinkProvider":{"resolveProvider":true},
                "renameProvider":{"prepareProvider":true,"workDoneProgress":true},
                "documentFormattingProvider":true,"documentRangeFormattingProvider":true,
                "semanticTokensProvider":{"legend":presentation::legend(&capabilities),"full":{"delta":true},"range":true,"workDoneProgress":true},
                "foldingRangeProvider":true,"selectionRangeProvider":true,"colorProvider":true,
                "executeCommandProvider":{"commands":["instar.fixAll","instar.organizeImports","instar.openConfiguration"]},
                "workspace": {
                    "workspaceFolders":{"supported":true,"changeNotifications":true},
                    "fileOperations": {
                        "didCreate":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]},
                        "willRename":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]},
                        "didRename":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]},
                        "didDelete":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]}
                    }
                }
            },
            "serverInfo":{"name":"instar","version":env!("CARGO_PKG_VERSION")}
        })).map_err(failure)
    }

    async fn initialized(&self, _: InitializedParams) {
        let result = async {
            messages::initialized(&self.client).await;

            if self.has_capability("/workspace/workspaceFolders")
                && let Some(folders) = self.client.workspace_folders().await?
            {
                self.connection
                    .control(move |state| {
                        for folder in folders {
                            state.add(&folder.uri)?;
                        }

                        Ok(())
                    })
                    .await?;
            }

            self.configuration(None).await?;
            self.register().await?;

            self.revalidate(true).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let mut registrations = self.registrations.lock().await;
        registrations.is_stopping = true;

        self.connection
            .control(|state| {
                state.projects.clear();

                Ok(())
            })
            .await?;

        if let Err(error) = registrations.unregister(&self.client).await {
            messages::warning(&self.client, &error).await;
        }

        self.has_shutdown.store(true, Ordering::Relaxed);

        Ok(())
    }

    async fn did_open(&self, parameters: DidOpenTextDocumentParams) {
        self.notify(
            parameters.text_document.uri.clone(),
            parameters,
            synchronization::open,
        )
        .await;
    }

    async fn did_change(&self, parameters: DidChangeTextDocumentParams) {
        self.notify(
            parameters
                .text_document
                .text_document_identifier
                .uri
                .clone(),
            parameters,
            synchronization::change,
        )
        .await;
    }

    async fn did_save(&self, parameters: DidSaveTextDocumentParams) {
        self.notify(
            parameters.text_document.uri.clone(),
            parameters,
            synchronization::save,
        )
        .await;
    }

    async fn did_close(&self, parameters: DidCloseTextDocumentParams) {
        self.notify(
            parameters.text_document.uri.clone(),
            parameters,
            |project, parameters| synchronization::close(project, &parameters),
        )
        .await;
    }

    async fn hover(&self, parameters: HoverParams) -> Result<Option<Hover>> {
        self.query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            suggestions::hover,
        )
        .await
    }

    async fn completion(&self, parameters: CompletionParams) -> Result<Option<CompletionResponse>> {
        self.query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            suggestions::completion,
        )
        .await
    }

    async fn completion_resolve(&self, parameters: CompletionItem) -> Result<CompletionItem> {
        let uri = resolved_uri(parameters.data.as_ref())?;

        self.query(
            Consistency::Snapshot,
            uri,
            None,
            parameters,
            suggestions::completion_resolve,
        )
        .await
    }

    async fn signature_help(
        &self,
        parameters: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        self.query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            suggestions::signature_help,
        )
        .await
    }

    async fn code_action(
        &self,
        parameters: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionResponse>>> {
        self.source(
            Consistency::Current,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| editing::code_action(project, &parameters),
        )
        .await
    }

    async fn inlay_hint(&self, parameters: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        self.source_or_query(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            presentation::inlay_hint_source,
            |project, frontend, parameters| {
                presentation::inlay_hint(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn inlay_hint_resolve(&self, parameters: InlayHint) -> Result<InlayHint> {
        let uri = resolved_uri(parameters.data.as_ref())?;

        self.source_or_query(
            Consistency::Snapshot,
            uri,
            None,
            parameters,
            presentation::inlay_hint_resolve_source,
            presentation::inlay_hint_resolve,
        )
        .await
    }

    async fn goto_definition(
        &self,
        parameters: DefinitionParams,
    ) -> Result<Option<DefinitionResponse>> {
        self.query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                navigation::goto_definition(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn goto_type_definition(
        &self,
        parameters: TypeDefinitionParams,
    ) -> Result<Option<TypeDefinitionResponse>> {
        self.query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                navigation::goto_type_definition(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn document_highlight(
        &self,
        parameters: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        self.source_or_query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            navigation::document_highlight_source,
            navigation::document_highlight,
        )
        .await
    }

    async fn document_symbol(
        &self,
        parameters: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.source(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| navigation::document_symbol(project, &parameters),
        )
        .await
    }

    async fn document_link(
        &self,
        parameters: DocumentLinkParams,
    ) -> Result<Option<Vec<DocumentLink>>> {
        self.source(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| navigation::document_link(project, &parameters),
        )
        .await
    }

    async fn document_link_resolve(&self, parameters: DocumentLink) -> Result<DocumentLink> {
        if parameters.target.is_some() {
            return Ok(parameters);
        }

        let uri = resolved_uri(parameters.data.as_ref())?;

        self.source_or_query(
            Consistency::Snapshot,
            uri,
            None,
            parameters,
            navigation::document_link_resolve_source,
            navigation::document_link_resolve,
        )
        .await
    }

    async fn prepare_rename(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResult>> {
        self.source_or_query(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            None,
            parameters,
            editing::prepare_rename_source,
            |project, frontend, parameters| editing::prepare_rename(project, frontend, &parameters),
        )
        .await
    }

    async fn rename(&self, parameters: RenameParams) -> Result<Option<WorkspaceEdit>> {
        self.source_or_query(
            Consistency::Current,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            editing::rename_source,
            editing::rename,
        )
        .await
    }

    async fn formatting(
        &self,
        parameters: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.format(
            parameters.text_document.uri,
            parameters.work_done_progress_params.work_done_token,
            None,
        )
        .await
    }

    async fn range_formatting(
        &self,
        parameters: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.format(
            parameters.text_document.uri,
            parameters.work_done_progress_params.work_done_token,
            Some(parameters.range),
        )
        .await
    }

    async fn semantic_tokens_full(
        &self,
        parameters: SemanticTokensParams,
    ) -> Result<Option<SemanticTokens>> {
        self.query(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                presentation::semantic_tokens_full(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn semantic_tokens_full_delta(
        &self,
        parameters: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensDeltaResponse>> {
        self.query(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                presentation::semantic_tokens_full_delta(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn semantic_tokens_range(
        &self,
        parameters: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokens>> {
        self.query(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                presentation::semantic_tokens_range(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn folding_range(
        &self,
        parameters: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>> {
        self.source(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| presentation::folding_range(project, &parameters),
        )
        .await
    }

    async fn selection_range(
        &self,
        parameters: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        self.source(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            presentation::selection_range,
        )
        .await
    }

    async fn document_color(
        &self,
        parameters: DocumentColorParams,
    ) -> Result<Vec<ColorInformation>> {
        self.source(
            Consistency::Snapshot,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| presentation::document_color(project, &parameters),
        )
        .await
    }

    async fn color_presentation(
        &self,
        parameters: ColorPresentationParams,
    ) -> Result<Vec<ColorPresentation>> {
        self.source(
            Consistency::Current,
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| presentation::color_presentation(project, &parameters),
        )
        .await
    }

    async fn references(&self, parameters: ReferenceParams) -> Result<Option<Vec<Location>>> {
        self.source_or_query(
            Consistency::Snapshot,
            parameters
                .text_document_position_params
                .text_document
                .uri
                .clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            navigation::references_source,
            navigation::references,
        )
        .await
    }

    async fn symbol(
        &self,
        parameters: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        let token = parameters.work_done_progress_params.work_done_token.clone();

        self.perform(
            Consistency::Snapshot,
            Priority::Interactive,
            token,
            |operation| async move {
                let roots = self.connection.control(|state| state.roots()).await?;
                let mut information = Vec::new();

                for root in roots {
                    let parameters = parameters.clone();

                    let result = self
                        .connection
                        .source(root, Arc::clone(&operation), move |project| {
                            navigation::symbol(project, &parameters)
                        })
                        .await?;

                    information.extend(result);
                }

                Ok(Some(WorkspaceSymbolResponse::SymbolInformationList(
                    information,
                )))
            },
        )
        .await
    }

    async fn did_change_configuration(&self, parameters: DidChangeConfigurationParams) {
        let result = async {
            if !self.configuration(Some(parameters.settings)).await? {
                return Ok(());
            }

            self.maintain(false).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn did_change_workspace_folders(&self, parameters: DidChangeWorkspaceFoldersParams) {
        let result = async {
            let removed = self
                .connection
                .control(move |state| {
                    let mut removed = Vec::new();

                    for folder in parameters.event.removed {
                        removed.extend(state.remove(&folder.uri)?);
                    }

                    for folder in parameters.event.added {
                        state.add(&folder.uri)?;
                    }

                    Ok(removed)
                })
                .await?;

            for uri in removed {
                self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }

            self.configuration(None).await?;

            self.maintain(true).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn did_change_watched_files(&self, parameters: DidChangeWatchedFilesParams) {
        let result = async {
            let paths = parameters
                .changes
                .into_iter()
                .map(|change| document::path(&change.uri))
                .collect::<Result<_>>()?;

            self.changed(paths).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn did_create_files(&self, parameters: CreateFilesParams) {
        let result = async {
            let paths = parameters
                .files
                .into_iter()
                .map(|file| document::path(&file.uri))
                .collect::<Result<_>>()?;

            self.changed(paths).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn did_rename_files(&self, parameters: RenameFilesParams) {
        let result = async {
            let removed = self
                .connection
                .control(move |state| files::renamed(state, parameters))
                .await?;

            for uri in removed {
                self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }

            self.maintain(false).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn did_delete_files(&self, parameters: DeleteFilesParams) {
        let result = async {
            let removed = self
                .connection
                .control(move |state| files::deleted(state, parameters))
                .await?;

            for uri in removed {
                self.client.publish_diagnostics(uri, Vec::new(), None).await;
            }

            self.maintain(false).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }

    async fn will_rename_files(
        &self,
        parameters: RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let roots = self.connection.control(|state| state.roots()).await?;
        let mut result = WorkspaceEdit::default();

        for root in roots {
            let parameters = parameters.clone();

            if let Some(edit) = self
                .perform(
                    Consistency::Current,
                    Priority::Interactive,
                    None,
                    |operation| {
                        self.connection.query(
                            Target::Uri(root),
                            operation,
                            move |project, frontend| {
                                files::will_rename(project, frontend, parameters)
                            },
                        )
                    },
                )
                .await?
            {
                if let Some(changes) = edit.changes {
                    result.changes.get_or_insert_default().extend(changes);
                }

                if let Some(documents) = edit.document_changes {
                    result
                        .document_changes
                        .get_or_insert_default()
                        .extend(documents);
                }
            }
        }

        Ok(Some(result))
    }

    async fn execute_command(&self, parameters: ExecuteCommandParams) -> Result<Option<Value>> {
        if parameters.command == "instar.openConfiguration" {
            if !self.has_capability("/window/showDocument/support") {
                return Err(Error::invalid_params(
                    "Client does not support opening documents.",
                ));
            }

            let roots = self.connection.control(|state| state.roots()).await?;

            if let Some(action) = messages::configuration(&self.client, &roots).await? {
                let root = roots
                    .into_iter()
                    .find(|root| root.as_str() == action.title)
                    .ok_or_else(|| {
                        Error::invalid_params("Client selected an unknown workspace.")
                    })?;

                let uri = document::uri(&document::path(&root)?.join("instar.toml"))?;

                let is_opened = self
                    .client
                    .show_document(ShowDocumentParams::new(uri, None, Some(true), None))
                    .await?;

                return Ok(Some(Value::Bool(is_opened)));
            }

            return Ok(None);
        }

        let kind = match parameters.command.as_str() {
            "instar.fixAll" => "source.fixAll.instar",
            "instar.organizeImports" => "source.organizeImports",
            _ => return Err(Error::invalid_params("Unknown Instar command.")),
        };

        if !self.has_capability("/workspace/applyEdit") {
            return Err(Error::invalid_params(
                "Client does not support workspace edits.",
            ));
        }

        let arguments = parameters.arguments.unwrap_or_default();

        let [argument] = arguments.as_slice() else {
            return Err(Error::invalid_params("Command expects one document URI."));
        };

        let uri: Uri = serde_json::from_value(argument.clone())
            .map_err(|error| Error::invalid_params(error.to_string()))?;

        let target = uri.clone();

        let edit = self
            .perform(
                Consistency::Current,
                Priority::Interactive,
                parameters.work_done_progress_params.work_done_token,
                |operation| {
                    self.connection.source(uri, operation, move |project| {
                        let module = project.module(&target)?;

                        editing::action(project, &module, kind)
                    })
                },
            )
            .await?;

        let result = self.client.apply_edit(edit).await?;

        if !result.applied {
            return Err(reason(
                result
                    .failure_reason
                    .unwrap_or_else(|| "Client declined workspace edit.".to_owned()),
            ));
        }

        Ok(Some(Value::Bool(true)))
    }
}
