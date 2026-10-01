//! Tokio language server with thread-confined Luau analysis and independent syntax requests.

mod bindings;
mod document;
mod features;
mod imports;
mod worker;
mod workspace;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use instar_core::{format, project::Project};
use serde_json::Value;
use tokio::sync::{Mutex, Notify, oneshot};

use tower_lsp_server::ls_types as lsp;

use tower_lsp_server::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::{Error, ErrorCode, Result},
    ls_types::{
        CompletionParams, CompletionResponse, DidChangeConfigurationParams,
        DidChangeTextDocumentParams, DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        DocumentFormattingParams, DocumentHighlight, DocumentHighlightParams, DocumentLink,
        DocumentLinkParams, DocumentSymbolParams, DocumentSymbolResponse, FoldingRange,
        FoldingRangeParams, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverParams,
        InitializeParams, InitializeResult, InitializedParams, Location, MessageType,
        PrepareRenameResponse, ReferenceParams, RenameParams, SelectionRange, SelectionRangeParams,
        SemanticTokensParams, SemanticTokensResult, SignatureHelp, SignatureHelpParams,
        TextDocumentPositionParams, TextDocumentSyncKind, TextEdit, Uri, WatchKind, WorkspaceEdit,
        request::{GotoTypeDefinitionParams, GotoTypeDefinitionResponse},
    },
};

type ProgressRequests = Arc<std::sync::Mutex<HashMap<lsp::ProgressToken, Arc<AtomicBool>>>>;

struct RequestGuard {
    cancelled: Arc<AtomicBool>,
    key: Option<lsp::ProgressToken>,
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
    preparing: BTreeMap<PathBuf, Arc<Notify>>,
    folders: BTreeSet<PathBuf>,
}

enum Response {
    Hover(Option<lsp::Hover>),
    Completion(lsp::CompletionResponse),
    Signature(Option<lsp::SignatureHelp>),
    Locations(Vec<lsp::Location>),
    Prepare(Option<lsp::PrepareRenameResponse>),
    Edit(lsp::WorkspaceEdit),
    Highlights(Vec<lsp::DocumentHighlight>),
    Hints(Vec<lsp::InlayHint>),
    Actions(lsp::CodeActionResponse),
    Links(Vec<lsp::DocumentLink>),
    Symbols(Vec<lsp::SymbolInformation>),
    Diagnostics(lsp::WorkspaceDiagnosticReport),
    Diagnostic(lsp::DocumentDiagnosticReport),
}

enum Command {
    Wake,

    Query {
        revision: u64,
        path: PathBuf,
        query: Query,
        reply: oneshot::Sender<std::result::Result<Response, QueryFailure>>,
    },

    Stop,
}

enum QueryFailure {
    ContentModified,
    Operation(io::Error),
}

struct Backend {
    client: Client,
    state: Arc<Mutex<Snapshot>>,
    revision: Arc<AtomicU64>,
    queued: Arc<AtomicBool>,
    pull_diagnostics: Arc<AtomicBool>,
    diagnostic_refresh: Arc<AtomicBool>,
    commands: mpsc::Sender<Command>,
    imports: mpsc::Sender<Option<imports::Command>>,
    threads: std::sync::Mutex<Vec<thread::JoinHandle<()>>>,
    watch: AtomicBool,
    versioned_edits: AtomicBool,
    flag_refresh: AtomicBool,
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

fn log_asset_warnings(client: &Client, runtime: &tokio::runtime::Handle, warnings: Vec<String>) {
    if warnings.is_empty() {
        return;
    }

    let client = client.clone();

    runtime.spawn(async move {
        for warning in warnings {
            client.log_message(MessageType::WARNING, warning).await;
        }
    });
}

fn workspace_capabilities() -> lsp::WorkspaceServerCapabilities {
    let operations = lsp::FileOperationRegistrationOptions {
        filters: vec![lsp::FileOperationFilter {
            scheme: Some("file".to_owned()),
            pattern: lsp::FileOperationPattern {
                glob: "**".to_owned(),
                ..lsp::FileOperationPattern::default()
            },
        }],
    };

    lsp::WorkspaceServerCapabilities {
        workspace_folders: Some(lsp::WorkspaceFoldersServerCapabilities {
            supported: Some(true),
            change_notifications: Some(lsp::OneOf::Left(true)),
        }),
        file_operations: Some(lsp::WorkspaceFileOperationsServerCapabilities {
            will_rename: Some(operations.clone()),
            did_rename: Some(operations.clone()),
            did_create: Some(operations.clone()),
            did_delete: Some(operations),
            ..lsp::WorkspaceFileOperationsServerCapabilities::default()
        }),
    }
}

fn capabilities(pull_diagnostics: bool) -> lsp::ServerCapabilities {
    lsp::ServerCapabilities {
        position_encoding: Some(lsp::PositionEncodingKind::UTF16),
        text_document_sync: Some(
            lsp::TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(
                    lsp::SaveOptions {
                        include_text: Some(false),
                    }
                    .into(),
                ),
                ..lsp::TextDocumentSyncOptions::default()
            }
            .into(),
        ),
        workspace: Some(workspace_capabilities()),
        hover_provider: Some(true.into()),
        completion_provider: Some(lsp::CompletionOptions {
            trigger_characters: Some(
                [".", ":", "\"", "'", "/", "@"]
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            ),
            ..lsp::CompletionOptions::default()
        }),
        signature_help_provider: Some(lsp::SignatureHelpOptions {
            trigger_characters: Some(["(", ","].into_iter().map(str::to_owned).collect()),
            ..lsp::SignatureHelpOptions::default()
        }),
        definition_provider: Some(lsp::OneOf::Left(true)),
        type_definition_provider: Some(true.into()),
        references_provider: Some(lsp::OneOf::Left(true)),
        declaration_provider: Some(lsp::DeclarationCapability::Simple(true)),
        implementation_provider: Some(true.into()),
        rename_provider: Some(lsp::OneOf::Right(lsp::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
        })),
        document_highlight_provider: Some(lsp::OneOf::Left(true)),
        document_link_provider: Some(lsp::DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: lsp::WorkDoneProgressOptions::default(),
        }),
        document_symbol_provider: Some(lsp::OneOf::Left(true)),
        document_formatting_provider: Some(lsp::OneOf::Left(true)),
        workspace_symbol_provider: Some(lsp::OneOf::Left(true)),
        code_action_provider: Some(
            lsp::CodeActionOptions {
                code_action_kinds: Some(vec![lsp::CodeActionKind::QUICKFIX]),
                ..lsp::CodeActionOptions::default()
            }
            .into(),
        ),
        inlay_hint_provider: Some(lsp::OneOf::Left(true)),
        color_provider: Some(true.into()),
        diagnostic_provider: pull_diagnostics.then(|| {
            lsp::DiagnosticServerCapabilities::Options(lsp::DiagnosticOptions {
                identifier: Some("instar".to_owned()),
                inter_file_dependencies: true,
                workspace_diagnostics: true,
                work_done_progress_options: lsp::WorkDoneProgressOptions {
                    work_done_progress: Some(true),
                },
            })
        }),
        semantic_tokens_provider: Some(
            lsp::SemanticTokensOptions {
                legend: lsp::SemanticTokensLegend {
                    token_types: bindings::SemanticKind::LEGEND.to_vec(),
                    token_modifiers: bindings::MODIFIER_LEGEND.to_vec(),
                },
                full: Some(lsp::SemanticTokensFullOptions::Bool(true)),
                ..lsp::SemanticTokensOptions::default()
            }
            .into(),
        ),
        ..lsp::ServerCapabilities::default()
    }
}

impl Backend {
    fn new(client: Client) -> Self {
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let revision = Arc::new(AtomicU64::new(0));
        let queued = Arc::new(AtomicBool::new(false));
        let pull_diagnostics = Arc::new(AtomicBool::new(false));
        let diagnostic_refresh = Arc::new(AtomicBool::new(false));
        let (commands, receiver) = mpsc::channel();
        let worker_state = Arc::clone(&state);
        let worker_revision = Arc::clone(&revision);
        let worker_queued = Arc::clone(&queued);
        let worker_pull_diagnostics = Arc::clone(&pull_diagnostics);
        let worker_diagnostic_refresh = Arc::clone(&diagnostic_refresh);
        let worker_client = client.clone();
        let runtime = tokio::runtime::Handle::current();

        // ponytail: one Luau worker serializes type queries; shard workspaces if contention warrants it.
        let thread = thread::spawn(move || {
            let mut worker = Worker::default();

            let mut diagnostic_deadline: Option<Instant> = None;

            loop {
                let command = match diagnostic_deadline {
                    Some(deadline) => match receiver
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    {
                        Ok(command) => command,

                        Err(mpsc::RecvTimeoutError::Timeout) => {
                            diagnostic_deadline = None;

                            Self::update_diagnostics(
                                &mut worker,
                                worker_client.clone(),
                                &worker_state,
                                &runtime,
                                worker_pull_diagnostics.load(Ordering::Acquire),
                                worker_diagnostic_refresh.load(Ordering::Acquire),
                            );

                            continue;
                        }

                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    },

                    None => match receiver.recv() {
                        Ok(command) => command,
                        Err(_) => break,
                    },
                };

                match command {
                    Command::Stop => break,

                    Command::Wake => {
                        worker_queued.store(false, Ordering::Release);
                        diagnostic_deadline = Some(Instant::now() + Duration::from_millis(40));
                    }

                    Command::Query {
                        revision,
                        path,
                        query,
                        reply,
                    } => {
                        if reply.is_closed() {
                            continue;
                        }

                        let snapshot = {
                            let snapshot = worker_state.blocking_lock();

                            if snapshot.revision != revision {
                                drop(reply.send(Err(QueryFailure::ContentModified)));
                                continue;
                            }

                            snapshot.clone()
                        };

                        let mut result = worker
                            .update(&snapshot, false)
                            .and_then(|()| worker.query(&path, &query))
                            .map_err(QueryFailure::Operation);

                        log_asset_warnings(&worker_client, &runtime, worker.take_asset_warnings());

                        if revision != worker_revision.load(Ordering::Acquire) {
                            result = Err(QueryFailure::ContentModified);
                        }

                        drop(reply.send(result));
                    }
                }
            }
        });

        let (imports, import_thread) = imports::start();

        let (workspace, workspace_thread) =
            workspace::start(client.clone(), tokio::runtime::Handle::current());

        Self {
            client,
            state,
            revision,
            queued,
            pull_diagnostics,
            diagnostic_refresh,
            commands,
            imports,
            threads: std::sync::Mutex::new(vec![thread, import_thread, workspace_thread]),
            watch: AtomicBool::new(false),
            flag_refresh: AtomicBool::new(false),
            versioned_edits: AtomicBool::new(false),
            workspace,
            progress_requests: Arc::default(),
            hint_types: AtomicBool::new(true),
            hint_parameters: AtomicBool::new(true),
            hint_requires: AtomicBool::new(false),
        }
    }

    async fn prepare_flags(&self, paths: Vec<PathBuf>) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }

        let (sync, warnings) = tokio::task::spawn_blocking(move || {
            let mut project = Project::new();
            let sync = project.prepare_fast_flags(&paths)?;

            Ok::<_, io::Error>((sync, project.take_asset_warnings()))
        })
        .await
        .map_err(|failure| error(&failure))?
        .map_err(|failure| error(&failure))?;

        for warning in warnings {
            self.client.log_message(MessageType::WARNING, warning).await;
        }

        if sync && !self.flag_refresh.swap(true, Ordering::AcqRel) {
            let client = self.client.clone();

            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_hours(12)).await;

                    match tokio::task::spawn_blocking(Project::refresh_fast_flag_cache).await {
                        Ok(Ok(warnings)) => {
                            for warning in warnings {
                                client.log_message(MessageType::WARNING, warning).await;
                            }
                        }

                        Ok(Err(failure)) => {
                            client
                                .log_message(MessageType::WARNING, failure.to_string())
                                .await;
                        }

                        Err(failure) => {
                            client
                                .log_message(MessageType::ERROR, failure.to_string())
                                .await;
                        }
                    }
                }
            });
        }

        Ok(())
    }

    fn update_diagnostics(
        worker: &mut Worker,
        client: Client,
        state: &Arc<Mutex<Snapshot>>,
        runtime: &tokio::runtime::Handle,
        pull: bool,
        refresh: bool,
    ) {
        if pull {
            if refresh {
                runtime.spawn(async move {
                    if let Err(failure) = client.workspace_diagnostic_refresh().await {
                        client
                            .log_message(MessageType::WARNING, failure.to_string())
                            .await;
                    }
                });
            }
        } else {
            let snapshot = state.blocking_lock().clone();

            let diagnostics = worker
                .update(&snapshot, true)
                .map(|()| worker.diagnostics.clone());

            log_asset_warnings(&client, runtime, worker.take_asset_warnings());

            runtime.spawn(Self::publish(
                client,
                Arc::clone(state),
                snapshot,
                diagnostics,
            ));
        }
    }

    async fn publish(
        client: Client,
        state: Arc<Mutex<Snapshot>>,
        snapshot: Snapshot,
        diagnostics: io::Result<BTreeMap<PathBuf, Vec<lsp::Diagnostic>>>,
    ) {
        let current = state.lock().await;

        if current.revision != snapshot.revision {
            return;
        }

        let mut diagnostics = match diagnostics {
            Ok(diagnostics) => diagnostics,

            Err(failure) => {
                client
                    .log_message(MessageType::ERROR, failure.to_string())
                    .await;

                return;
            }
        };

        for (path, document) in &snapshot.documents {
            let items = diagnostics.remove(path).unwrap_or_default();

            client
                .publish_diagnostics(document.uri.clone(), items, Some(document.version))
                .await;
        }
    }

    fn changed(&self, state: &mut Snapshot) {
        state.revision += 1;
        self.revision.store(state.revision, Ordering::Release);

        if (!self.pull_diagnostics.load(Ordering::Acquire)
            || self.diagnostic_refresh.load(Ordering::Acquire))
            && !self.queued.swap(true, Ordering::AcqRel)
        {
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

        Ok((state.epoch, document))
    }

    async fn ready_document(&self, uri: &Uri) -> Result<(u64, Arc<Document>)> {
        let path = document::path(uri).map_err(|failure| error(&failure))?;
        let state = self.ready_state(Some(&path)).await;

        let document = state
            .documents
            .get(&path)
            .cloned()
            .ok_or_else(|| Error::invalid_params("document is not open"))?;

        Ok((state.revision, document))
    }

    async fn ready_state(
        &self,
        path: Option<&std::path::Path>,
    ) -> tokio::sync::MutexGuard<'_, Snapshot> {
        loop {
            let state = self.state.lock().await;

            let preparing = match path {
                Some(path) => state.preparing.get(path),
                None => state.preparing.values().next(),
            }
            .cloned();

            if let Some(preparing) = preparing {
                let notified = preparing.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                drop(state);
                notified.await;
            } else {
                return state;
            }
        }
    }

    async fn dispatch(&self, revision: u64, document: &Document, query: Query) -> Result<Response> {
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
            Ok(value) => Ok(value),

            Err(QueryFailure::ContentModified) => Err(Error::content_modified()),
            Err(QueryFailure::Operation(failure)) => Err(error(&failure)),
        }
    }

    async fn query_at(
        &self,
        params: TextDocumentPositionParams,
        make: impl FnOnce(u32, u32) -> Query + Send,
    ) -> Result<Response> {
        let (revision, document) = self.ready_document(&params.text_document.uri).await?;

        let (line, column) = document
            .byte_position(params.position)
            .map_err(|failure| error(&failure))?;

        let query = make(line, column);

        if !matches!(
            query,
            Query::Definition(..)
                | Query::Declaration(..)
                | Query::Implementation(..)
                | Query::Completion(..)
        ) {
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

    async fn resolve(
        &self,
        document: Arc<Document>,
        request: imports::Request,
    ) -> Result<Response> {
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

        result.map_err(|failure| error(&failure))
    }

    async fn syntax<T: Send + 'static>(
        &self,
        uri: Uri,
        operation: impl FnOnce(&Document) -> io::Result<T> + Send + 'static,
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
            return Err(Error::content_modified());
        }

        Ok(value)
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        self.configure_hints(
            params
                .initialization_options
                .as_ref()
                .unwrap_or(&Value::Null),
        )?;

        let pull_diagnostics = params
            .capabilities
            .text_document
            .as_ref()
            .is_some_and(|document| document.diagnostic.is_some());

        let workspace = params.capabilities.workspace.as_ref();

        self.pull_diagnostics
            .store(pull_diagnostics, Ordering::Release);

        self.diagnostic_refresh.store(
            workspace
                .and_then(|workspace| workspace.diagnostics.as_ref())
                .and_then(|diagnostics| diagnostics.refresh_support)
                .unwrap_or(false),
            Ordering::Release,
        );

        self.versioned_edits.store(
            workspace
                .and_then(|workspace| workspace.workspace_edit.as_ref())
                .and_then(|edit| edit.document_changes)
                .unwrap_or(false),
            Ordering::Release,
        );

        self.watch.store(
            workspace
                .and_then(|workspace| workspace.did_change_watched_files.as_ref())
                .and_then(|watch| watch.dynamic_registration)
                .unwrap_or(false),
            Ordering::Release,
        );

        let mut state = self.state.lock().await;

        for folder in params.workspace_folders.unwrap_or_default() {
            state
                .folders
                .insert(document::path(&folder.uri).map_err(|failure| error(&failure))?);
        }

        #[expect(
            deprecated,
            reason = "clients without workspace folders still send rootUri"
        )]
        let root = params.root_uri;

        if state.folders.is_empty()
            && let Some(root) = root
        {
            state
                .folders
                .insert(document::path(&root).map_err(|failure| error(&failure))?);
        }

        let folders = state.folders.iter().cloned().collect();
        drop(state);
        self.prepare_flags(folders).await?;

        Ok(InitializeResult {
            capabilities: capabilities(pull_diagnostics),
            server_info: Some(lsp::ServerInfo {
                name: "instar".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.watch.load(Ordering::Acquire) {
            let options = lsp::DidChangeWatchedFilesRegistrationOptions {
                watchers: vec![lsp::FileSystemWatcher {
                    glob_pattern: lsp::GlobPattern::String("**/*".to_owned()),
                    kind: Some(WatchKind::Create | WatchKind::Change | WatchKind::Delete),
                }],
            };

            let registration = serde_json::to_value(options).map(|options| lsp::Registration {
                id: "instar.files".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: Some(options),
            });

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
                let path = document.path.clone();
                let preparing = Arc::new(Notify::new());

                {
                    let mut state = self.state.lock().await;
                    state.documents.insert(path.clone(), Arc::new(document));
                    state.preparing.insert(path.clone(), Arc::clone(&preparing));
                }

                let result = self.prepare_flags(vec![path.clone()]).await;
                let mut state = self.state.lock().await;

                if state
                    .preparing
                    .get(&path)
                    .is_some_and(|current| Arc::ptr_eq(current, &preparing))
                {
                    state.preparing.remove(&path);

                    if result.is_err() {
                        state.documents.remove(&path);
                    }

                    self.changed(&mut state);
                }

                drop(state);
                preparing.notify_waiters();

                if let Err(failure) = result {
                    self.client
                        .log_message(MessageType::ERROR, failure.to_string())
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

            if let Some(preparing) = state.preparing.remove(&path) {
                preparing.notify_waiters();
            }

            self.changed(&mut state);

            if !self.pull_diagnostics.load(Ordering::Acquire) {
                self.client
                    .publish_diagnostics(params.text_document.uri, Vec::new(), None)
                    .await;
            }
        }
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        let mut state = self.state.lock().await;
        self.changed(&mut state);
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        enum Outcome {
            Excluded,
            Unchanged,
            Changed(String),
        }

        let (epoch, document) = self.document(&params.text_document.uri).await?;
        let path = document.path.clone();
        let source = document.text.clone();

        let formatted = tokio::task::spawn_blocking(move || {
            let mut project = Project::new();

            let Some(options) = project.format_options(&path)? else {
                return Ok::<_, io::Error>(Outcome::Excluded);
            };

            let formatted = format::source(&source, &options)?;

            Ok(if formatted == source {
                Outcome::Unchanged
            } else {
                Outcome::Changed(formatted)
            })
        })
        .await
        .map_err(|failure| error(&failure))?
        .map_err(|failure| error(&failure))?;

        let state = self.state.lock().await;

        if state.epoch != epoch
            || state
                .documents
                .get(&document.path)
                .is_none_or(|current| !Arc::ptr_eq(current, &document))
        {
            return Err(Error::content_modified());
        }

        match formatted {
            Outcome::Excluded => Err(Error::invalid_params(format!(
                "{} is excluded by format filters",
                document.path.display()
            ))),

            Outcome::Unchanged => Ok(None),

            Outcome::Changed(formatted) => Ok(Some(vec![TextEdit::new(
                document.range(0, document.text.len()),
                formatted,
            )])),
        }
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
        let mut folders = self.state.lock().await.folders.clone();

        for folder in params.event.removed {
            if let Ok(path) = document::path(&folder.uri) {
                folders.remove(&path);
            }
        }

        for folder in params.event.added {
            if let Ok(path) = document::path(&folder.uri) {
                folders.insert(path);
            }
        }

        if let Err(failure) = self.prepare_flags(folders.iter().cloned().collect()).await {
            self.client
                .log_message(MessageType::ERROR, failure.to_string())
                .await;

            return;
        }

        let mut state = self.state.lock().await;
        state.folders = folders;

        state.epoch += 1;
        self.changed(&mut state);
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let Response::Hover(result) = self
            .query_at(params.text_document_position_params, Query::Hover)
            .await?
        else {
            return Err(error(&"unexpected hover response"));
        };

        Ok(result)
    }

    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        let Response::Completion(result) = self
            .query_at(params.text_document_position, Query::Completion)
            .await?
        else {
            return Err(error(&"unexpected completion response"));
        };

        Ok(Some(result))
    }

    async fn signature_help(&self, params: SignatureHelpParams) -> Result<Option<SignatureHelp>> {
        let Response::Signature(result) = self
            .query_at(params.text_document_position_params, Query::Signature)
            .await?
        else {
            return Err(error(&"unexpected signature response"));
        };

        Ok(result)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> Result<Option<GotoDefinitionResponse>> {
        let Response::Locations(result) = self
            .query_at(params.text_document_position_params, Query::Definition)
            .await?
        else {
            return Err(error(&"unexpected definition response"));
        };

        Ok(Some(GotoDefinitionResponse::Array(result)))
    }

    async fn goto_declaration(
        &self,
        params: lsp::request::GotoDeclarationParams,
    ) -> Result<Option<lsp::request::GotoDeclarationResponse>> {
        let Response::Locations(result) = self
            .query_at(params.text_document_position_params, Query::Declaration)
            .await?
        else {
            return Err(error(&"unexpected declaration response"));
        };

        Ok(Some(GotoDefinitionResponse::Array(result)))
    }

    async fn goto_implementation(
        &self,
        params: lsp::request::GotoImplementationParams,
    ) -> Result<Option<lsp::request::GotoImplementationResponse>> {
        let Response::Locations(result) = self
            .query_at(params.text_document_position_params, Query::Implementation)
            .await?
        else {
            return Err(error(&"unexpected implementation response"));
        };

        Ok(Some(GotoDefinitionResponse::Array(result)))
    }

    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> Result<Option<GotoTypeDefinitionResponse>> {
        let Response::Locations(result) = self
            .query_at(params.text_document_position_params, Query::TypeDefinition)
            .await?
        else {
            return Err(error(&"unexpected type definition response"));
        };

        Ok(Some(GotoDefinitionResponse::Array(result)))
    }

    async fn references(&self, params: ReferenceParams) -> Result<Option<Vec<Location>>> {
        let Response::Locations(result) = self
            .query_at(params.text_document_position, |line, column| {
                Query::References(line, column, params.context.include_declaration)
            })
            .await?
        else {
            return Err(error(&"unexpected references response"));
        };

        Ok(Some(result))
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResponse>> {
        let Response::Prepare(result) = self.query_at(params, Query::Prepare).await? else {
            return Err(error(&"unexpected prepare rename response"));
        };

        Ok(result)
    }

    async fn rename(&self, params: RenameParams) -> Result<Option<WorkspaceEdit>> {
        let Response::Edit(result) = self
            .query_at(params.text_document_position, |line, column| {
                Query::Rename(line, column, params.new_name)
            })
            .await?
        else {
            return Err(error(&"unexpected rename response"));
        };

        self.compatible_edit(result).map(Some)
    }

    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        let Response::Highlights(result) = self
            .query_at(params.text_document_position_params, Query::Highlights)
            .await?
        else {
            return Err(error(&"unexpected highlights response"));
        };

        Ok(Some(result))
    }

    async fn document_link(&self, params: DocumentLinkParams) -> Result<Option<Vec<DocumentLink>>> {
        let (_, document) = self.ready_document(&params.text_document.uri).await?;

        let Response::Links(result) = self.resolve(document, imports::Request::Links).await? else {
            return Err(error(&"unexpected links response"));
        };

        Ok(Some(result))
    }

    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> Result<Option<SemanticTokensResult>> {
        self.syntax(params.text_document.uri, |document| {
            Ok(Some(SemanticTokensResult::Tokens(document.tokens())))
        })
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        self.syntax(params.text_document.uri, |document| {
            Ok(Some(DocumentSymbolResponse::Nested(
                document.symbols().to_vec(),
            )))
        })
        .await
    }

    async fn folding_range(&self, params: FoldingRangeParams) -> Result<Option<Vec<FoldingRange>>> {
        self.syntax(params.text_document.uri, |document| {
            Ok(Some(document.folds().to_vec()))
        })
        .await
    }

    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        self.syntax(params.text_document.uri, move |document| {
            document.selections(&params.positions).map(Some)
        })
        .await
    }

    async fn symbol(
        &self,
        params: lsp::WorkspaceSymbolParams,
    ) -> Result<Option<lsp::WorkspaceSymbolResponse>> {
        let Response::Symbols(result) = self
            .workspace_query(
                workspace::Request::Symbols(params.query),
                params.work_done_progress_params.work_done_token,
            )
            .await?
        else {
            return Err(error(&"unexpected workspace symbols response"));
        };

        Ok(Some(lsp::WorkspaceSymbolResponse::Flat(result)))
    }

    async fn will_rename_files(
        &self,
        params: lsp::RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        let Response::Edit(edit) = self
            .workspace_query(
                workspace::Request::Rename(Self::rename_paths(&params)?),
                None,
            )
            .await?
        else {
            return Err(error(&"unexpected file rename response"));
        };

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

        let (revision, document) = self.ready_document(&params.text_document.uri).await?;

        let Response::Actions(mut actions) = self
            .dispatch(revision, &document, Query::Actions(params.range))
            .await?
        else {
            return Err(error(&"unexpected code actions response"));
        };

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
        let (revision, document) = self.ready_document(&params.text_document.uri).await?;

        let Response::Hints(result) = self
            .dispatch(
                revision,
                &document,
                Query::Hints(
                    params.range,
                    self.hint_types.load(Ordering::Acquire),
                    self.hint_parameters.load(Ordering::Acquire),
                    self.hint_requires.load(Ordering::Acquire),
                ),
            )
            .await?
        else {
            return Err(error(&"unexpected inlay hints response"));
        };

        Ok(Some(result))
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

        let Response::Diagnostics(result) = self
            .workspace_query(
                workspace::Request::Diagnostics(previous),
                params.work_done_progress_params.work_done_token,
            )
            .await?
        else {
            return Err(error(&"unexpected workspace diagnostics response"));
        };

        Ok(lsp::WorkspaceDiagnosticReportResult::Report(result))
    }

    async fn diagnostic(
        &self,
        params: lsp::DocumentDiagnosticParams,
    ) -> Result<lsp::DocumentDiagnosticReportResult> {
        let path = document::path(&params.text_document.uri).map_err(|failure| error(&failure))?;

        let Response::Diagnostic(result) = self
            .workspace_query(
                workspace::Request::Document(path, params.previous_result_id),
                params.work_done_progress_params.work_done_token,
            )
            .await?
        else {
            return Err(error(&"unexpected document diagnostics response"));
        };

        Ok(lsp::DocumentDiagnosticReportResult::Report(result))
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
                // ponytail: unbounded admission; add per-class limits if clients flood requests.
                .concurrency_level(usize::MAX)
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
            && let Some(flag) = requests.get(&params.token)
        {
            flag.store(true, Ordering::Release);
        }

        std::future::ready(())
    }

    async fn progress(&self, token: &lsp::ProgressToken, value: lsp::WorkDoneProgress) {
        self.client
            .send_notification::<lsp::notification::Progress>(lsp::ProgressParams {
                token: token.clone(),
                value: lsp::ProgressParamsValue::WorkDone(value),
            })
            .await;
    }

    async fn workspace_query(
        &self,
        request: workspace::Request,
        token: Option<lsp::ProgressToken>,
    ) -> Result<Response> {
        let cancelled = Arc::new(AtomicBool::new(false));

        let key = token.clone();

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

        let snapshot = self.ready_state(None).await.clone();
        let revision = snapshot.revision;
        let (reply, mut response) = oneshot::channel();
        let (progress, mut updates) = tokio::sync::mpsc::unbounded_channel();

        if let Some(token) = &token {
            self.progress(
                token,
                lsp::WorkDoneProgress::Begin(lsp::WorkDoneProgressBegin {
                    title: "Instar workspace".to_owned(),
                    cancellable: Some(true),
                    message: None,
                    percentage: Some(0),
                }),
            )
            .await;
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
                    let percentage = u32::try_from(done.saturating_mul(100).checked_div(total).unwrap_or(0))
                        .unwrap_or(100);
                    if previous != Some(percentage) && percentage < 100 {
                        previous = Some(percentage);
                        if let Some(token) = &token {
                            self.progress(token, lsp::WorkDoneProgress::Report(lsp::WorkDoneProgressReport {
                                cancellable: Some(true),
                                percentage: Some(percentage),
                                message: Some(format!("{done}/{total} modules")),
                            })).await;
                        }
                    }
                }
            }
        };

        if let Some(token) = &token {
            self.progress(
                token,
                lsp::WorkDoneProgress::End(lsp::WorkDoneProgressEnd::default()),
            )
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

        Ok(value)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::poll_fn, task::Poll};

    #[derive(Clone, Copy)]
    enum Change {
        Save,
        OtherDocument,
        Document,
        Configuration,
        Close,
    }

    #[test]
    fn formatting_invalidates_only_for_its_document_or_configuration() {
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let (service, _socket) = LspService::new(Backend::new);
                let backend = service.inner();
                backend.pull_diagnostics.store(true, Ordering::Release);
                let uri = document::uri(&std::env::temp_dir().join("formatting.luau")).unwrap();
                let other = document::uri(&std::env::temp_dir().join("other.luau")).unwrap();
                let identifier = lsp::TextDocumentIdentifier::new(uri.clone());

                for change in [
                    Change::Save,
                    Change::OtherDocument,
                    Change::Document,
                    Change::Configuration,
                    Change::Close,
                ] {
                    {
                        let mut state = backend.state.lock().await;

                        for uri in [&uri, &other] {
                            let document =
                                Document::new(uri.clone(), 1, "local value=1\n".into()).unwrap();

                            state
                                .documents
                                .insert(document.path.clone(), Arc::new(document));
                        }

                        backend.changed(&mut state);
                    }

                    let (release, blocked) = mpsc::channel::<()>();
                    let (started, ready) = oneshot::channel();

                    let blocking = tokio::task::spawn_blocking(move || {
                        started.send(()).unwrap();
                        let _ = blocked.recv();
                    });

                    ready.await.unwrap();

                    let formatting = backend.formatting(DocumentFormattingParams {
                        text_document: identifier.clone(),
                        options: lsp::FormattingOptions {
                            tab_size: 4,
                            insert_spaces: true,
                            ..Default::default()
                        },
                        work_done_progress_params: lsp::WorkDoneProgressParams::default(),
                    });

                    tokio::pin!(formatting);

                    assert!(
                        poll_fn(|context| Poll::Ready(formatting.as_mut().poll(context)))
                            .await
                            .is_pending()
                    );

                    match change {
                        Change::Save => {
                            backend
                                .did_save(DidSaveTextDocumentParams {
                                    text_document: identifier.clone(),
                                    text: None,
                                })
                                .await;
                        }

                        Change::OtherDocument | Change::Document => {
                            backend
                                .did_change(DidChangeTextDocumentParams {
                                    text_document: lsp::VersionedTextDocumentIdentifier::new(
                                        if matches!(change, Change::OtherDocument) {
                                            other.clone()
                                        } else {
                                            uri.clone()
                                        },
                                        2,
                                    ),
                                    content_changes: vec![lsp::TextDocumentContentChangeEvent {
                                        range: None,
                                        range_length: None,
                                        text: "local value=2\n".into(),
                                    }],
                                })
                                .await;
                        }

                        Change::Configuration => backend.refresh().await,

                        Change::Close => {
                            backend
                                .did_close(DidCloseTextDocumentParams {
                                    text_document: identifier.clone(),
                                })
                                .await;
                        }
                    }

                    drop(release);
                    blocking.await.unwrap();
                    let result = formatting.await;

                    if matches!(change, Change::Save | Change::OtherDocument) {
                        assert_eq!(result.unwrap().unwrap()[0].new_text, "local value = 1\n");
                    } else {
                        assert_eq!(result.unwrap_err().code, ErrorCode::ContentModified);
                    }
                }

                backend.shutdown().await.unwrap();
            });
    }
}
