//! Tokio language server with thread-confined Luau analysis and independent syntax requests.

mod bindings;
mod document;
mod imports;
mod worker;

use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::sync::{Mutex, oneshot};

use tower_lsp_server::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::{Error, ErrorCode, Result},
    ls_types::{
        CompletionParams, CompletionResponse, DidChangeConfigurationParams,
        DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        DocumentHighlight, DocumentHighlightParams, DocumentLink, DocumentLinkParams,
        DocumentSymbolParams, DocumentSymbolResponse, FoldingRange, FoldingRangeParams,
        GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams, InitializeParams,
        InitializeResult, InitializedParams, Location, MessageType, PrepareRenameResponse,
        ReferenceParams, RenameParams, SelectionRange, SelectionRangeParams, SemanticTokensParams,
        SemanticTokensResult, SignatureHelp, SignatureHelpParams, TextDocumentPositionParams,
        TextDocumentSyncKind, Uri, WatchKind, WorkspaceEdit,
        request::{GotoTypeDefinitionParams, GotoTypeDefinitionResponse},
    },
};

use document::Document;
use worker::{Query, Worker};

#[derive(Clone, Default)]
struct Snapshot {
    revision: u64,
    epoch: u64,
    documents: BTreeMap<PathBuf, Arc<Document>>,
    folders: BTreeSet<PathBuf>,
}

enum Command {
    Wake,

    Query {
        revision: u64,
        path: PathBuf,
        query: Query,
        reply: oneshot::Sender<std::result::Result<Value, String>>,
    },

    Stop,
}

struct Backend {
    client: Client,
    state: Arc<Mutex<Snapshot>>,
    revision: Arc<AtomicU64>,
    queued: Arc<AtomicBool>,
    commands: mpsc::Sender<Command>,
    imports: mpsc::Sender<Option<imports::Command>>,
    threads: std::sync::Mutex<Vec<thread::JoinHandle<()>>>,
    watch: AtomicBool,
    versioned_edits: AtomicBool,
}

fn error(message: &impl ToString) -> Error {
    Error {
        code: ErrorCode::InternalError,
        message: message.to_string().into(),
        data: None,
    }
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_json::from_value(value).map_err(|failure| error(&failure))
}

impl Backend {
    fn new(client: Client) -> Self {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let revision = Arc::new(AtomicU64::new(0));
        let queued = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_revision = Arc::clone(&revision);
        let worker_queued = Arc::clone(&queued);
        let worker_client = client.clone();
        let runtime = tokio::runtime::Handle::current();

        // ponytail: one Luau worker serializes type queries; shard workspaces if contention warrants it.
        let thread = thread::spawn(move || {
            let mut worker = Worker::default();

            while let Ok(command) = receiver.recv() {
                if matches!(command, Command::Stop) {
                    break;
                }

                if matches!(command, Command::Wake) {
                    worker_queued.store(false, Ordering::Release);
                }

                if let Command::Query {
                    revision,
                    ref reply,
                    ..
                } = command
                {
                    if reply.is_closed() {
                        continue;
                    }

                    if revision != worker_revision.load(Ordering::Acquire) {
                        if let Command::Query { reply, .. } = command {
                            drop(reply.send(Err("content modified".into())));
                        }

                        continue;
                    }
                }

                let snapshot = worker_state.blocking_lock().clone();
                let checked = worker.update(&snapshot);

                match command {
                    Command::Query {
                        path, query, reply, ..
                    } => {
                        let result = checked
                            .and_then(|()| worker.query(&path, &query))
                            .map_err(|e| e.to_string());

                        let result = if snapshot.revision == worker_revision.load(Ordering::Acquire)
                        {
                            result
                        } else {
                            Err("content modified".into())
                        };

                        drop(reply.send(result));
                    }

                    Command::Wake => {
                        let client = worker_client.clone();
                        let state = Arc::clone(&worker_state);
                        let diagnostics = worker.diagnostics.clone();

                        runtime.spawn(Self::publish(client, state, snapshot, diagnostics, checked));
                    }

                    Command::Stop => break,
                }
            }
        });

        let (imports, import_thread) = imports::start();

        Self {
            client,
            state,
            revision,
            queued,
            commands,
            imports,
            threads: std::sync::Mutex::new(vec![thread, import_thread]),
            watch: AtomicBool::new(false),
            versioned_edits: AtomicBool::new(false),
        }
    }

    async fn publish(
        client: Client,
        state: Arc<Mutex<Snapshot>>,
        snapshot: Snapshot,
        diagnostics: BTreeMap<PathBuf, Vec<Value>>,
        checked: io::Result<()>,
    ) {
        let current = state.lock().await;

        if current.revision != snapshot.revision {
            return;
        }

        if let Err(failure) = checked {
            client
                .log_message(MessageType::ERROR, failure.to_string())
                .await;
        }

        for (path, document) in &snapshot.documents {
            let items = diagnostics.get(path).cloned().unwrap_or_default();

            match serde_json::from_value(json!(items)) {
                Ok(items) => {
                    client
                        .publish_diagnostics(document.uri.clone(), items, Some(document.version))
                        .await;
                }

                Err(failure) => {
                    client
                        .log_message(MessageType::ERROR, failure.to_string())
                        .await;
                }
            }
        }
    }

    fn changed(&self, state: &mut Snapshot) {
        state.revision += 1;
        self.revision.store(state.revision, Ordering::Release);

        if !self.queued.swap(true, Ordering::AcqRel) {
            drop(self.commands.send(Command::Wake));
        }
    }

    async fn refresh(&self) {
        let mut state = self.state.lock().await;
        state.epoch += 1;
        self.changed(&mut state);
    }

    async fn document(&self, uri: &Uri) -> Result<(u64, Arc<Document>)> {
        let path = document::path(uri).map_err(|failure| error(&failure))?;
        let state = self.state.lock().await;

        let document = state
            .documents
            .get(&path)
            .cloned()
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        Ok((state.revision, document))
    }

    async fn dispatch<T: DeserializeOwned>(
        &self,
        revision: u64,
        document: &Document,
        query: Query,
    ) -> Result<T> {
        let (reply, receiver) = oneshot::channel();

        self.commands
            .send(Command::Query {
                revision,
                path: document.path.clone(),
                query,
                reply,
            })
            .map_err(|failure| error(&failure))?;

        match receiver.await.map_err(|failure| error(&failure))? {
            Ok(value) => decode(value),

            Err(message) if message == "content modified" => Err(Error {
                code: ErrorCode::ContentModified,
                message: message.into(),
                data: None,
            }),

            Err(message) => Err(error(&message)),
        }
    }

    async fn query_at<T: DeserializeOwned>(
        &self,
        params: TextDocumentPositionParams,
        make: impl FnOnce(u32, u32) -> Query + Send,
    ) -> Result<T> {
        let (revision, document) = self.document(&params.text_document.uri).await?;

        let (line, column) = document
            .byte_position(params.position)
            .map_err(|failure| error(&failure))?;

        let query = make(line, column);

        if !matches!(query, Query::Definition(..) | Query::Completion(..)) {
            return self.dispatch(revision, &document, query).await;
        }

        let offset = document
            .offset(params.position)
            .map_err(|failure| error(&failure))?;

        let source = Arc::clone(&document);

        let (import, query) = tokio::task::spawn_blocking(move || {
            Ok::<_, io::Error>((imports::at(&source, &query, offset)?, query))
        })
        .await
        .map_err(|failure| error(&failure))?
        .map_err(|failure| error(&failure))?;

        if revision != self.revision.load(Ordering::Acquire) {
            return Err(Error::content_modified());
        }

        if let Some(request) = import {
            return self.resolve(document, request).await;
        }

        self.dispatch(revision, &document, query).await
    }

    async fn resolve<T: DeserializeOwned>(
        &self,
        document: Arc<Document>,
        request: imports::Request,
    ) -> Result<T> {
        let snapshot = self.state.lock().await.clone();

        if snapshot
            .documents
            .get(&document.path)
            .is_none_or(|current| !Arc::ptr_eq(current, &document))
        {
            return Err(Error::content_modified());
        }

        let revision = snapshot.revision;
        let (reply, receiver) = oneshot::channel();

        self.imports
            .send(Some(imports::Command {
                snapshot,
                document,
                request,
                reply,
            }))
            .map_err(|failure| error(&failure))?;

        let result = receiver.await.map_err(|failure| error(&failure))?;

        if revision != self.revision.load(Ordering::Acquire) {
            return Err(Error::content_modified());
        }

        decode(result.map_err(|failure| error(&failure))?)
    }

    async fn syntax<T: DeserializeOwned>(
        &self,
        uri: Uri,
        operation: impl FnOnce(&Document) -> io::Result<Value> + Send + 'static,
    ) -> Result<T> {
        let (_, document) = self.document(&uri).await?;
        let source = Arc::clone(&document);

        let value = tokio::task::spawn_blocking(move || operation(&source))
            .await
            .map_err(|failure| error(&failure))?
            .map_err(|failure| error(&failure))?;

        if self
            .state
            .lock()
            .await
            .documents
            .get(&document.path)
            .is_none_or(|current| !Arc::ptr_eq(current, &document))
        {
            return Err(Error {
                code: ErrorCode::ContentModified,
                message: "content modified".into(),
                data: None,
            });
        }

        decode(value)
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let initialization = serde_json::to_value(&params).map_err(|failure| error(&failure))?;
        let capabilities = &initialization["capabilities"];

        self.versioned_edits.store(
            capabilities
                .pointer("/workspace/workspaceEdit/documentChanges")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            Ordering::Release,
        );

        self.watch.store(
            capabilities
                .pointer("/workspace/didChangeWatchedFiles/dynamicRegistration")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            Ordering::Release,
        );

        let mut state = self.state.lock().await;

        for folder in params.workspace_folders.unwrap_or_default() {
            state
                .folders
                .insert(document::path(&folder.uri).map_err(|failure| error(&failure))?);
        }

        if state.folders.is_empty()
            && let Some(root) = initialization.get("rootUri").and_then(Value::as_str)
        {
            let root = root.parse::<Uri>().map_err(|failure| error(&failure))?;

            state
                .folders
                .insert(document::path(&root).map_err(|failure| error(&failure))?);
        }

        decode(
            json!({"serverInfo": {"name": "instar", "version": env!("CARGO_PKG_VERSION")}, "capabilities": {
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": TextDocumentSyncKind::INCREMENTAL, "save": {"includeText": false}},
                "workspace": {"workspaceFolders": {"supported": true, "changeNotifications": true}},
                "hoverProvider": true, "completionProvider": {"triggerCharacters": [".", ":", "\"", "'", "/", "@"]},
                "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                "definitionProvider": true, "typeDefinitionProvider": true, "referencesProvider": true,
                "renameProvider": {"prepareProvider": true}, "documentHighlightProvider": true,
                "documentLinkProvider": {"resolveProvider": false}, "documentSymbolProvider": true,
                "foldingRangeProvider": true, "selectionRangeProvider": true,
                "semanticTokensProvider": {"legend": {"tokenTypes": bindings::SemanticKind::LEGEND,
                    "tokenModifiers": bindings::MODIFIER_LEGEND}, "full": true}
            }}),
        )
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.watch.load(Ordering::Acquire) {
            let registration = decode(
                json!({"id": "instar.files", "method": "workspace/didChangeWatchedFiles", "registerOptions": {"watchers": [{"globPattern": "**/*", "kind": WatchKind::Create | WatchKind::Change | WatchKind::Delete}]}}),
            );

            match registration {
                Ok(registration) => {
                    if let Err(failure) = self.client.register_capability(vec![registration]).await
                    {
                        self.client
                            .log_message(MessageType::WARNING, failure.to_string())
                            .await;
                    }
                }

                Err(failure) => {
                    self.client
                        .log_message(MessageType::ERROR, failure.to_string())
                        .await;
                }
            }
        }
    }

    async fn shutdown(&self) -> Result<()> {
        self.commands
            .send(Command::Stop)
            .map_err(|failure| error(&failure))?;

        self.imports.send(None).map_err(|failure| error(&failure))?;
        let threads = std::mem::take(&mut *self.threads.lock().map_err(|failure| error(&failure))?);

        tokio::task::spawn_blocking(move || {
            for thread in threads {
                thread
                    .join()
                    .map_err(|_| io::Error::other("language worker panicked"))?;
            }

            Ok::<_, io::Error>(())
        })
        .await
        .map_err(|failure| error(&failure))?
        .map_err(|failure| error(&failure))?;

        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let item = params.text_document;

        match Document::new(item.uri, item.version, item.text) {
            Ok(document) => {
                let mut state = self.state.lock().await;

                state
                    .documents
                    .insert(document.path.clone(), Arc::new(document));

                self.changed(&mut state);
            }

            Err(failure) => {
                self.client
                    .log_message(MessageType::ERROR, failure.to_string())
                    .await;
            }
        }
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let result = async {
            let path = document::path(&params.text_document.uri)?;
            let mut state = self.state.lock().await;

            let current = state
                .documents
                .get(&path)
                .ok_or_else(|| io::Error::other("change for unopened document"))?;

            if params.text_document.version <= current.version {
                return Ok::<_, io::Error>(());
            }

            let mut next = Document::new(
                current.uri.clone(),
                params.text_document.version,
                current.text.clone(),
            )?;

            for change in params.content_changes {
                if let Some(range) = change.range {
                    let start = next.offset(range.start)?;
                    let end = next.offset(range.end)?;

                    if start > end {
                        return Err(io::Error::other("reversed edit range"));
                    }

                    next.text.replace_range(start..end, &change.text);
                } else {
                    next.text = change.text;
                }

                next = Document::new(next.uri, next.version, next.text)?;
            }

            state.documents.insert(path, Arc::new(next));
            self.changed(&mut state);

            Ok(())
        }
        .await;

        if let Err(failure) = result {
            self.client
                .log_message(MessageType::ERROR, failure.to_string())
                .await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        if let Ok(path) = document::path(&params.text_document.uri) {
            let mut state = self.state.lock().await;
            state.documents.remove(&path);
            self.changed(&mut state);

            self.client
                .publish_diagnostics(params.text_document.uri, Vec::new(), None)
                .await;
        }
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        let mut state = self.state.lock().await;
        self.changed(&mut state);
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        self.refresh().await;
    }

    async fn did_change_configuration(&self, _: DidChangeConfigurationParams) {
        self.refresh().await;
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        let mut state = self.state.lock().await;

        for folder in params.event.removed {
            if let Ok(path) = document::path(&folder.uri) {
                state.folders.remove(&path);
            }
        }

        for folder in params.event.added {
            if let Ok(path) = document::path(&folder.uri) {
                state.folders.insert(path);
            }
        }

        state.epoch += 1;
        self.changed(&mut state);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        self.query_at(params.text_document_position_params, Query::Hover)
            .await
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        self.query_at(params.text_document_position, Query::Completion)
            .await
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        self.query_at(params.text_document_position_params, Query::Signature)
            .await
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        self.query_at(params.text_document_position_params, Query::Definition)
            .await
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        self.query_at(params.text_document_position_params, Query::TypeDefinition)
            .await
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        self.query_at(params.text_document_position, |line, column| {
            Query::References(line, column, params.context.include_declaration)
        })
        .await
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        self.query_at(params, Query::Prepare).await
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let mut result: Option<WorkspaceEdit> = self
            .query_at(params.text_document_position, |line, column| {
                Query::Rename(line, column, params.new_name)
            })
            .await?;

        if !self.versioned_edits.load(Ordering::Acquire)
            && let Some(edit) = &mut result
        {
            let Some(tower_lsp_server::ls_types::DocumentChanges::Edits(changes)) =
                edit.document_changes.take()
            else {
                return Err(error(&"unexpected rename edit representation"));
            };

            edit.changes = Some(
                changes
                    .into_iter()
                    .map(|change| {
                        let edits = change
                            .edits
                            .into_iter()
                            .map(|edit| match edit {
                                tower_lsp_server::ls_types::OneOf::Left(edit) => edit,
                                tower_lsp_server::ls_types::OneOf::Right(edit) => edit.text_edit,
                            })
                            .collect();

                        (change.text_document.uri, edits)
                    })
                    .collect(),
            );
        }

        Ok(result)
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        self.query_at(params.text_document_position_params, Query::Highlights)
            .await
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let (_, document) = self.document(&params.text_document.uri).await?;

        self.resolve(document, imports::Request::Links).await
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        self.syntax(params.text_document.uri, |document| Ok(document.tokens()))
            .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.syntax(params.text_document.uri, |document| Ok(document.symbols()))
            .await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        self.syntax(params.text_document.uri, |document| Ok(document.folds()))
            .await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        self.syntax(params.text_document.uri, move |document| {
            document.selections(&params.positions)
        })
        .await
    }
}

/// Serves the language server over standard input and output.
///
/// # Errors
/// Returns runtime construction errors.
pub fn run() -> io::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let (service, socket) = LspService::new(Backend::new);

            Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
                .serve(service)
                .await;
        });

    Ok(())
}
