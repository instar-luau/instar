//! Tokio language server with thread-confined Luau analysis and independent syntax requests.

mod bindings;
mod document;
mod features;
mod imports;
mod worker;
mod workspace;

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

use tower_lsp_server::ls_types as lsp;

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

type ProgressRequests = Arc<std::sync::Mutex<BTreeMap<String, Arc<AtomicBool>>>>;

struct RequestGuard {
    cancelled: Arc<AtomicBool>,
    key: Option<String>,
    requests: ProgressRequests,
}

impl Drop for RequestGuard {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);

        if let Some(key) = &self.key
            && let Ok(mut requests) = self.requests.lock()
            && requests
                .get(key)
                .is_some_and(|flag| Arc::ptr_eq(flag, &self.cancelled))
        {
            requests.remove(key);
        }
    }
}

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
    workspace: mpsc::Sender<Option<workspace::Command>>,
    progress_requests: ProgressRequests,
    hint_types: AtomicBool,
    hint_parameters: AtomicBool,
    hint_requires: AtomicBool,
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
        let (workspace, workspace_thread) = workspace::start();

        Self {
            client,
            state,
            revision,
            queued,
            commands,
            imports,
            threads: std::sync::Mutex::new(vec![thread, import_thread, workspace_thread]),
            watch: AtomicBool::new(false),
            versioned_edits: AtomicBool::new(false),
            workspace,
            progress_requests: Arc::default(),
            hint_types: AtomicBool::new(true),
            hint_parameters: AtomicBool::new(true),
            hint_requires: AtomicBool::new(false),
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
        self.configure_hints(&initialization["initializationOptions"])?;

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
                "workspace": {"workspaceFolders": {"supported": true, "changeNotifications": true},
                    "fileOperations": {
                        "willRename": {"filters": [{"scheme": "file", "pattern": {"glob": "**"}}]},
                        "didRename": {"filters": [{"scheme": "file", "pattern": {"glob": "**"}}]},
                        "didCreate": {"filters": [{"scheme": "file", "pattern": {"glob": "**"}}]},
                        "didDelete": {"filters": [{"scheme": "file", "pattern": {"glob": "**"}}]}
                    }},
                "hoverProvider": true, "completionProvider": {"triggerCharacters": [".", ":", "\"", "'", "/", "@"]},
                "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
                "definitionProvider": true, "typeDefinitionProvider": true, "referencesProvider": true,
                "renameProvider": {"prepareProvider": true}, "documentHighlightProvider": true,
                "documentLinkProvider": {"resolveProvider": false}, "documentSymbolProvider": true,
                "foldingRangeProvider": true, "selectionRangeProvider": true,
                "workspaceSymbolProvider": true,
                "codeActionProvider": {"codeActionKinds": ["quickfix"]},
                "inlayHintProvider": true, "colorProvider": true,
                "diagnosticProvider": {"identifier": "instar", "interFileDependencies": true, "workspaceDiagnostics": true, "workDoneProgress": true},
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

        self.workspace
            .send(None)
            .map_err(|failure| error(&failure))?;

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

    async fn did_change_configuration(&self, params: DidChangeConfigurationParams) {
        if let Err(failure) = self.configure_hints(&params.settings) {
            self.client
                .log_message(MessageType::ERROR, failure.to_string())
                .await;
        }

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
        let result: Option<WorkspaceEdit> = self
            .query_at(params.text_document_position, |line, column| {
                Query::Rename(line, column, params.new_name)
            })
            .await?;

        result.map(|edit| self.compatible_edit(edit)).transpose()
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

    async fn symbol(
        &self,
        params: lsp::WorkspaceSymbolParams,
    ) -> Result<Option<lsp::WorkspaceSymbolResponse>> {
        self.workspace_query(
            workspace::Request::Symbols(params.query),
            params.work_done_progress_params.work_done_token,
        )
        .await
    }

    async fn will_rename_files(
        &self,
        params: lsp::RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let edit = self
            .workspace_query(
                workspace::Request::Rename(Self::rename_paths(&params)?),
                None,
            )
            .await?;

        self.compatible_edit(edit).map(Some)
    }

    async fn did_rename_files(&self, params: lsp::RenameFilesParams) {
        let result = async {
            let renames = Self::rename_paths(&params)?;
            let mut state = self.state.lock().await;
            let mut documents = BTreeMap::new();

            for (path, document) in &state.documents {
                let new_path = workspace::remap(path, &renames);

                let document = if new_path == *path {
                    Arc::clone(document)
                } else {
                    Arc::new(
                        Document::new(
                            document::uri(&new_path).map_err(|failure| error(&failure))?,
                            document.version,
                            document.text.clone(),
                        )
                        .map_err(|failure| error(&failure))?,
                    )
                };

                documents.insert(new_path, document);
            }

            state.documents = documents;
            state.epoch += 1;
            self.changed(&mut state);

            Ok::<_, Error>(())
        }
        .await;

        if let Err(failure) = result {
            self.client
                .log_message(MessageType::ERROR, failure.to_string())
                .await;
        }
    }

    async fn did_create_files(&self, _: lsp::CreateFilesParams) {
        self.refresh().await;
    }

    async fn did_delete_files(&self, _: lsp::DeleteFilesParams) {
        self.refresh().await;
    }

    async fn code_action(
        &self,
        params: lsp::CodeActionParams,
    ) -> Result<Option<lsp::CodeActionResponse>> {
        if params
            .context
            .only
            .as_ref()
            .is_some_and(|kinds| !kinds.iter().any(|kind| kind.as_str() == "quickfix"))
        {
            return Ok(Some(Vec::new()));
        }

        let (revision, document) = self.document(&params.text_document.uri).await?;

        let mut actions: lsp::CodeActionResponse = self
            .dispatch(revision, &document, Query::Actions(params.range))
            .await?;

        for action in &mut actions {
            if let lsp::CodeActionOrCommand::CodeAction(action) = action
                && let Some(edit) = action.edit.take()
            {
                action.edit = Some(self.compatible_edit(edit)?);
            }
        }

        Ok(Some(actions))
    }

    async fn inlay_hint(
        &self,
        params: lsp::InlayHintParams,
    ) -> Result<Option<Vec<lsp::InlayHint>>> {
        let (revision, document) = self.document(&params.text_document.uri).await?;

        self.dispatch(
            revision,
            &document,
            Query::Hints(
                params.range,
                self.hint_types.load(Ordering::Acquire),
                self.hint_parameters.load(Ordering::Acquire),
                self.hint_requires.load(Ordering::Acquire),
            ),
        )
        .await
    }

    async fn document_color(
        &self,
        params: lsp::DocumentColorParams,
    ) -> Result<Vec<lsp::ColorInformation>> {
        self.syntax(params.text_document.uri, |document| {
            Ok(document.features().colors(document))
        })
        .await
    }

    async fn color_presentation(
        &self,
        params: lsp::ColorPresentationParams,
    ) -> Result<Vec<lsp::ColorPresentation>> {
        self.syntax(params.text_document.uri, move |document| {
            Ok(document
                .features()
                .presentation(document, params.range, params.color))
        })
        .await
    }

    async fn workspace_diagnostic(
        &self,
        params: lsp::WorkspaceDiagnosticParams,
    ) -> Result<lsp::WorkspaceDiagnosticReportResult> {
        let previous = params
            .previous_result_ids
            .into_iter()
            .map(|item| (item.uri.to_string(), item.value))
            .collect();

        self.workspace_query(
            workspace::Request::Diagnostics(previous),
            params.work_done_progress_params.work_done_token,
        )
        .await
    }

    async fn diagnostic(
        &self,
        params: lsp::DocumentDiagnosticParams,
    ) -> Result<lsp::DocumentDiagnosticReportResult> {
        let path = document::path(&params.text_document.uri).map_err(|failure| error(&failure))?;

        self.workspace_query(
            workspace::Request::Document(path, params.previous_result_id),
            params.work_done_progress_params.work_done_token,
        )
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
            let (service, socket) = LspService::build(Backend::new)
                .custom_method("window/workDoneProgress/cancel", Backend::cancel_progress)
                .finish();

            Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
                .serve(service)
                .await;
        });

    Ok(())
}

impl Backend {
    fn configure_hints(&self, settings: &Value) -> Result<()> {
        if let Some(hints) = settings.get("instar").unwrap_or(settings).get("inlayHints") {
            for (name, flag) in [
                ("types", &self.hint_types),
                ("parameters", &self.hint_parameters),
                ("requires", &self.hint_requires),
            ] {
                if let Some(value) = hints.get(name) {
                    let value = value.as_bool().ok_or_else(|| {
                        Error::invalid_params(format!("inlayHints.{name} must be boolean"))
                    })?;

                    flag.store(value, Ordering::Release);
                }
            }
        }

        Ok(())
    }

    #[expect(
        clippy::needless_pass_by_value,
        reason = "the notification router requires owned deserialized parameters"
    )]
    fn cancel_progress(&self, params: lsp::WorkDoneProgressCancelParams) -> std::future::Ready<()> {
        if let Ok(requests) = self.progress_requests.lock()
            && let Some(flag) =
                requests.get(&serde_json::to_string(&params.token).unwrap_or_default())
        {
            flag.store(true, Ordering::Release);
        }

        std::future::ready(())
    }

    async fn workspace_query<T: DeserializeOwned>(
        &self,
        request: workspace::Request,
        token: Option<lsp::ProgressToken>,
    ) -> Result<T> {
        let cancelled = Arc::new(AtomicBool::new(false));

        let key = token
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|failure| error(&failure))?;

        if let Some(key) = &key {
            let mut requests = self
                .progress_requests
                .lock()
                .map_err(|failure| error(&failure))?;

            if requests.contains_key(key) {
                return Err(Error::invalid_params("progress token already in use"));
            }

            requests.insert(key.clone(), Arc::clone(&cancelled));
        }

        let _guard = RequestGuard {
            cancelled: Arc::clone(&cancelled),
            key,
            requests: Arc::clone(&self.progress_requests),
        };

        let snapshot = self.state.lock().await.clone();
        let revision = snapshot.revision;
        let (reply, mut response) = oneshot::channel();
        let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();

        if let Some(token) = &token {
            self.client.send_notification::<lsp::notification::Progress>(decode(json!({"token": token, "value": {"kind": "begin", "title": "Instar workspace", "cancellable": true, "percentage": 0}}))?).await;
        }

        self.workspace
            .send(Some(workspace::Command {
                snapshot,
                request,
                cancelled,
                progress,
                reply,
            }))
            .map_err(|failure| error(&failure))?;

        let mut previous = None;

        let result = loop {
            tokio::select! {
                result = &mut response => break result.map_err(|failure| error(&failure))?,
                Some((done, total)) = updates.recv() => {
                    let percentage = done.saturating_mul(100).checked_div(total).unwrap_or(0);
                    if previous != Some(percentage) && percentage < 100 {
                        previous = Some(percentage);
                        if let Some(token) = &token {
                            self.client.send_notification::<lsp::notification::Progress>(decode(json!({"token": token, "value": {"kind": "report", "cancellable": true, "percentage": percentage, "message": format!("{done}/{total} modules")}}))?).await;
                        }
                    }
                }
            }
        };

        if let Some(token) = token {
            self.client
                .send_notification::<lsp::notification::Progress>(decode(
                    json!({"token": token, "value": {"kind": "end"}}),
                )?)
                .await;
        }

        let value = result.map_err(|failure| {
            if failure.kind() == io::ErrorKind::Interrupted {
                Error::request_cancelled()
            } else {
                error(&failure)
            }
        })?;

        if revision != self.revision.load(Ordering::Acquire) {
            return Err(Error::content_modified());
        }

        decode(value)
    }

    fn compatible_edit(&self, mut edit: WorkspaceEdit) -> Result<WorkspaceEdit> {
        if !self.versioned_edits.load(Ordering::Acquire)
            && let Some(changes) = edit.document_changes.take()
        {
            let lsp::DocumentChanges::Edits(changes) = changes else {
                return Err(Error::invalid_params(
                    "client does not support document changes",
                ));
            };

            edit.changes = Some(
                changes
                    .into_iter()
                    .map(|change| {
                        (
                            change.text_document.uri,
                            change
                                .edits
                                .into_iter()
                                .map(|edit| match edit {
                                    lsp::OneOf::Left(edit) => edit,
                                    lsp::OneOf::Right(edit) => edit.text_edit,
                                })
                                .collect(),
                        )
                    })
                    .collect(),
            );
        }

        Ok(edit)
    }
}

impl Backend {
    fn rename_paths(params: &lsp::RenameFilesParams) -> Result<Vec<(PathBuf, PathBuf)>> {
        params
            .files
            .iter()
            .map(|file| {
                let old = file
                    .old_uri
                    .parse::<Uri>()
                    .map_err(|failure| error(&failure))?;

                let new = file
                    .new_uri
                    .parse::<Uri>()
                    .map_err(|failure| error(&failure))?;

                Ok((
                    document::path(&old).map_err(|failure| error(&failure))?,
                    document::path(&new).map_err(|failure| error(&failure))?,
                ))
            })
            .collect()
    }
}
