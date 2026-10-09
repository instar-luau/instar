use std::{
    collections::{BTreeMap, BTreeSet},
    future::{Future, Ready, ready},
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use instar_analysis::Options;
use instar_core::project::Change;
use serde_json::json;
use tokio::task::JoinHandle;

use tower_lsp_server::{
    Client, LanguageServer, NotCancellable, OngoingProgress, Unbounded,
    gen_lsp_types::{
        CompletionOptions, CreateFilesParams, DeleteFilesParams, DiagnosticOptions,
        DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, DocumentLinkOptions, DocumentOnTypeFormattingOptions,
        FileOperationFilter, FileOperationOptions, FileOperationPattern, FileOperationPatternKind,
        FileOperationRegistrationOptions, InitializeParams, InitializeResult, InitializedParams,
        InlayHintOptions, MessageType, PositionEncodingKind, ProgressToken, Registration,
        RenameFilesParams, RenameOptions, ServerCapabilities, ServerInfo, SignatureHelpOptions,
        TextDocumentContentOptions, TextDocumentContentParams, TextDocumentContentResult,
        TextDocumentSyncKind, TextDocumentSyncOptions, Uri, WorkspaceFolders,
        WorkspaceFoldersServerCapabilities, WorkspaceOptions,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use crate::{
    diagnostics::{self, Activity, Diagnostics},
    location,
    workers::Workers,
    workspace::{self, Workspace},
};

pub(crate) struct Session {
    client: Client,
    pub(crate) workspace: Arc<Mutex<Workspace>>,
    pub(crate) workers: Arc<Workers>,
    pub(crate) queries: Arc<Workers>,
    pub(crate) options: Options,
    watching: AtomicBool,
    pub(crate) shutdown: Arc<AtomicBool>,
    pub(crate) diagnostics: Arc<Diagnostics>,
    publication: Mutex<Option<JoinHandle<()>>>,
    published: Arc<Mutex<BTreeSet<Uri>>>,
    progress: AtomicBool,
    progress_task: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    pub(crate) fn new(
        client: Client,
        workers: Arc<Workers>,
        queries: Arc<Workers>,
        options: Options,
        shutdown: Arc<AtomicBool>,
        diagnostics: Arc<Diagnostics>,
    ) -> Self {
        Self {
            client,
            workspace: Arc::new(Mutex::new(Workspace::default())),
            workers,
            queries,
            options,
            watching: AtomicBool::new(false),
            shutdown,
            diagnostics,
            publication: Mutex::new(None),
            published: Arc::new(Mutex::new(BTreeSet::new())),
            progress: AtomicBool::new(false),
            progress_task: Mutex::new(None),
        }
    }

    pub(crate) async fn open(&self, parameters: DidOpenTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .open(parameters);

        self.schedule();

        self.report(result).await;
    }

    pub(crate) async fn change(&self, parameters: DidChangeTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .change(parameters);

        self.schedule();

        self.report(result).await;
    }

    pub(crate) async fn close(&self, parameters: DidCloseTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .close(parameters);

        self.schedule();

        self.report(result).await;
    }

    pub(crate) async fn save(&self, parameters: DidSaveTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .save(parameters);

        self.schedule();

        self.report(result).await;
    }

    pub(crate) fn watched(&self, parameters: DidChangeWatchedFilesParams) -> Ready<()> {
        self.files(parameters.changes.into_iter().map(|change| change.uri));

        ready(())
    }

    pub(crate) fn folders(&self, parameters: DidChangeWorkspaceFoldersParams) -> Ready<()> {
        let mut workspace = self.workspace.lock().expect("workspace lock");

        for folder in parameters.event.removed {
            match workspace::path(&folder.uri) {
                Ok(path) => {
                    workspace.roots.remove(&path);

                    workspace
                        .snapshot
                        .change(Change::Disk(path))
                        .expect("absolute workspace folder");
                }

                Err(error) => tracing::error!(%error, "Invalid workspace folder"),
            }
        }

        for folder in parameters.event.added {
            match workspace::path(&folder.uri) {
                Ok(path) => {
                    workspace.roots.insert(path.clone());

                    workspace
                        .snapshot
                        .change(Change::Disk(path))
                        .expect("absolute workspace folder");
                }

                Err(error) => tracing::error!(%error, "Invalid workspace folder"),
            }
        }

        drop(workspace);
        self.schedule();

        ready(())
    }

    pub(crate) fn configuration(&self, _: DidChangeConfigurationParams) -> Ready<()> {
        self.workspace.lock().expect("workspace lock").invalidate();
        self.schedule();

        ready(())
    }

    pub(crate) fn content(
        &self,
        parameters: TextDocumentContentParams,
    ) -> Ready<Result<TextDocumentContentResult>> {
        let uri = parameters.uri;

        ready(
            self.workspace
                .lock()
                .expect("workspace lock")
                .content(&uri)
                .map(|text| TextDocumentContentResult { text }),
        )
    }

    pub(crate) fn created(&self, parameters: CreateFilesParams) -> Ready<()> {
        self.files(parameters.files.into_iter().map(|file| file.uri));

        ready(())
    }

    pub(crate) fn deleted(&self, parameters: DeleteFilesParams) -> Ready<()> {
        self.files(parameters.files.into_iter().map(|file| file.uri));

        ready(())
    }

    pub(crate) fn renamed(&self, parameters: RenameFilesParams) -> Ready<()> {
        for file in parameters.files {
            if let Err(error) = self
                .workspace
                .lock()
                .expect("workspace lock")
                .renamed(&file.old_uri, &file.new_uri)
            {
                tracing::error!(%error, "Invalid rename notification");
            }
        }

        self.schedule();

        ready(())
    }

    fn files(&self, files: impl Iterator<Item = Uri>) {
        for uri in files {
            let result = workspace::path(&uri).and_then(|path| {
                self.workspace
                    .lock()
                    .expect("workspace lock")
                    .snapshot
                    .change(Change::Disk(path))
                    .map_err(|error| failure(&error))
            });

            if let Err(error) = result {
                tracing::error!(%error, "Invalid filesystem notification");
            }
        }

        self.schedule();
    }

    fn schedule(&self) {
        if self.shutdown.load(Ordering::Relaxed) {
            return;
        }

        let mut publication = self.publication.lock().expect("diagnostic publication");

        if let Some(publication) = publication.take() {
            publication.abort();
        }

        let workspace = self.workspace.lock().expect("workspace lock");

        if workspace.diagnostic_capabilities.is_some() {
            *publication = workspace
                .diagnostic_refresh
                .then(|| self.refresh_diagnostics());

            return;
        }

        *publication = Some(self.publish_diagnostics(&workspace));
    }

    fn publish_diagnostics(&self, workspace: &Workspace) -> JoinHandle<()> {
        let snapshot = workspace.snapshot.clone();
        let revision = snapshot.revision();
        let documents = workspace.documents();

        let locations = workspace.locations();

        let options = Options {
            cancellation: self.options.cancellation.child(),
            ..self.options.clone()
        };

        let response = self.diagnostics.submit(
            snapshot,
            documents.keys().cloned().collect(),
            workspace.roots.iter().cloned().collect(),
            workspace.encoding.clone(),
            options,
        );

        let workspace = Arc::clone(&self.workspace);
        let client = self.client.clone();
        let published = Arc::clone(&self.published);

        tokio::spawn(async move {
            let result = response
                .await
                .unwrap_or_else(|_| Err(io::Error::other("diagnostic worker stopped")));

            if workspace
                .lock()
                .expect("workspace lock")
                .snapshot
                .revision()
                != revision
            {
                return;
            }

            let report = match result {
                Ok(report) => report,

                Err(error) if documents.is_empty() => {
                    client
                        .log_message(MessageType::Error, error.to_string())
                        .await;

                    BTreeMap::new()
                }

                Err(error) => documents
                    .keys()
                    .map(|path| (path.clone(), vec![diagnostics::failure(&error.to_string())]))
                    .collect(),
            };

            let mut messages = Vec::new();
            let mut current = BTreeSet::new();

            for (path, mut diagnostics) in report {
                diagnostics::remap(&locations, &mut diagnostics);

                match locations
                    .get(&path)
                    .cloned()
                    .map_or_else(|| location::uri(&path), Ok)
                {
                    Ok(uri) => {
                        current.insert(uri.clone());
                        messages.push((uri, diagnostics, documents.get(&path).copied()));
                    }

                    Err(error) => {
                        client
                            .log_message(MessageType::Error, error.to_string())
                            .await;
                    }
                }
            }

            let previous = published.lock().expect("published diagnostics").clone();

            for uri in previous.difference(&current) {
                messages.push((uri.clone(), Vec::new(), None));
            }

            for (uri, diagnostics, version) in messages {
                if workspace
                    .lock()
                    .expect("workspace lock")
                    .snapshot
                    .revision()
                    != revision
                {
                    return;
                }

                published
                    .lock()
                    .expect("published diagnostics")
                    .insert(uri.clone());

                client.publish_diagnostics(uri, diagnostics, version).await;
            }

            *published.lock().expect("published diagnostics") = current;
        })
    }

    fn refresh_diagnostics(&self) -> JoinHandle<()> {
        let client = self.client.clone();

        tokio::spawn(async move {
            if let Err(error) = client.workspace_diagnostic_refresh().await {
                tracing::error!(%error, "Diagnostic refresh failed");
            }
        })
    }

    async fn report(&self, result: Result<()>) {
        if let Err(error) = result {
            self.client
                .log_message(MessageType::Error, error.to_string())
                .await;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.diagnostics.stop();

        if let Some(publication) = self
            .publication
            .lock()
            .expect("diagnostic publication")
            .take()
        {
            publication.abort();
        }

        if let Some(progress) = self.progress_task.lock().expect("progress task").take() {
            progress.abort();
        }
    }
}

impl LanguageServer for Session {
    fn initialize(
        &self,
        parameters: InitializeParams,
    ) -> impl Future<Output = Result<InitializeResult>> + Send {
        let roots = match parameters
            .workspace_folders_initialize_params
            .workspace_folders
        {
            Some(WorkspaceFolders::WorkspaceFolderList(folders)) => folders
                .iter()
                .map(|folder| workspace::path(&folder.uri))
                .collect::<Result<BTreeSet<_>>>(),

            _ => Ok(BTreeSet::new()),
        };

        let roots = match roots {
            Ok(roots) => roots,
            Err(error) => return ready(Err(error)),
        };

        let encoding = parameters
            .capabilities
            .general
            .as_ref()
            .and_then(|general| general.position_encodings.as_ref())
            .and_then(|encodings| {
                encodings.iter().find(|encoding| {
                    matches!(
                        encoding,
                        PositionEncodingKind::UTF8
                            | PositionEncodingKind::UTF16
                            | PositionEncodingKind::UTF32
                    )
                })
            })
            .cloned()
            .unwrap_or(PositionEncodingKind::UTF16);

        let watching = parameters
            .capabilities
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.did_change_watched_files.as_ref())
            .and_then(|watching| watching.dynamic_registration)
            .unwrap_or(false);

        self.watching.store(watching, Ordering::Relaxed);

        self.progress.store(
            parameters
                .capabilities
                .window
                .as_ref()
                .and_then(|window| window.work_done_progress)
                .unwrap_or(false),
            Ordering::Relaxed,
        );

        let mut workspace = self.workspace.lock().expect("workspace lock");
        workspace.roots = roots;
        workspace.encoding = encoding.clone();

        workspace.configure(&parameters.capabilities);

        let files = FileOperationRegistrationOptions {
            filters: vec![FileOperationFilter {
                scheme: Some("file".to_owned()),
                pattern: FileOperationPattern {
                    glob: "**/*".to_owned(),
                    matches: None,
                    options: None,
                },
            }],
        };

        ready(Ok(InitializeResult {
            capabilities: ServerCapabilities {
                position_encoding: Some(encoding),
                text_document_sync: Some(
                    TextDocumentSyncOptions {
                        open_close: Some(true),
                        change: Some(TextDocumentSyncKind::Incremental),
                        save: Some(true.into()),
                        ..TextDocumentSyncOptions::default()
                    }
                    .into(),
                ),
                diagnostic_provider: Some(
                    DiagnosticOptions {
                        inter_file_dependencies: true,
                        workspace_diagnostics: true,
                        ..DiagnosticOptions::default()
                    }
                    .into(),
                ),
                document_formatting_provider: Some(true.into()),
                document_range_formatting_provider: Some(true.into()),
                document_on_type_formatting_provider: Some(DocumentOnTypeFormattingOptions {
                    first_trigger_character: "\n".to_owned(),
                    more_trigger_character: Some(vec![";".to_owned()]),
                }),
                hover_provider: Some(true.into()),
                completion_provider: Some(CompletionOptions {
                    trigger_characters: Some(
                        [".", ":", "\"", "'", "/", "@"]
                            .into_iter()
                            .map(str::to_owned)
                            .collect(),
                    ),
                    resolve_provider: Some(true),
                    ..CompletionOptions::default()
                }),
                signature_help_provider: Some(SignatureHelpOptions {
                    trigger_characters: Some(vec!["(".to_owned(), ",".to_owned()]),
                    ..SignatureHelpOptions::default()
                }),
                definition_provider: Some(true.into()),
                type_definition_provider: Some(true.into()),
                implementation_provider: Some(true.into()),
                call_hierarchy_provider: Some(true.into()),
                type_hierarchy_provider: Some(true.into()),
                document_symbol_provider: Some(true.into()),
                workspace_symbol_provider: Some(true.into()),
                references_provider: Some(true.into()),
                document_highlight_provider: Some(true.into()),
                rename_provider: Some(
                    RenameOptions {
                        prepare_provider: Some(true),
                        ..RenameOptions::default()
                    }
                    .into(),
                ),
                code_action_provider: Some(true.into()),
                semantic_tokens_provider: workspace.tokens.provider().map(Into::into),
                folding_range_provider: Some(true.into()),
                selection_range_provider: Some(true.into()),
                document_link_provider: Some(DocumentLinkOptions::default()),
                inlay_hint_provider: Some(
                    InlayHintOptions {
                        resolve_provider: workspace.hints.deferred_tooltips.then_some(true),
                        ..InlayHintOptions::default()
                    }
                    .into(),
                ),
                workspace: Some(WorkspaceOptions {
                    file_operations: Some(FileOperationOptions {
                        did_create: Some(files.clone()),
                        did_rename: Some(files.clone()),
                        will_rename: Some(FileOperationRegistrationOptions {
                            filters: vec![FileOperationFilter {
                                scheme: Some("file".to_owned()),
                                pattern: FileOperationPattern {
                                    glob: "**/*.{lua,luau}".to_owned(),
                                    matches: Some(FileOperationPatternKind::File),
                                    options: None,
                                },
                            }],
                        }),
                        did_delete: Some(files),
                        ..FileOperationOptions::default()
                    }),
                    text_document_content: Some(
                        TextDocumentContentOptions {
                            schemes: vec!["instar".to_owned(), "untitled".to_owned()],
                        }
                        .into(),
                    ),
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(true.into()),
                    }),
                }),
                ..ServerCapabilities::default()
            },
            server_info: Some(ServerInfo {
                name: "Instar".to_owned(),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            }),
        }))
    }

    async fn initialized(&self, _: InitializedParams) {
        if self.watching.swap(false, Ordering::Relaxed) {
            let registration = Registration {
                id: "instar-files".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: Some(json!({"watchers": [{"globPattern": "**/*"}]})),
            };

            self.report(self.client.register_capability(vec![registration]).await)
                .await;
        }

        if self.progress.swap(false, Ordering::Relaxed) {
            let client = self.client.clone();
            let mut activity = self.diagnostics.activity();

            *self.progress_task.lock().expect("progress task") = Some(tokio::spawn(async move {
                let mut generation = 0_u64;
                let mut progress: Option<OngoingProgress<Unbounded, NotCancellable>> = None;

                loop {
                    let state = *activity.borrow_and_update();

                    if state == Activity::Running && progress.is_none() {
                        generation += 1;
                        let token: ProgressToken = format!("instar-analysis-{generation}").into();

                        if client
                            .create_work_done_progress(token.clone())
                            .await
                            .is_ok()
                        {
                            progress = Some(
                                client
                                    .progress(token, "Analyzing Luau documents")
                                    .begin()
                                    .await,
                            );
                        }
                    } else if state != Activity::Running
                        && let Some(progress) = progress.take()
                    {
                        progress.finish().await;
                    }

                    if state == Activity::Stopped || activity.changed().await.is_err() {
                        break;
                    }
                }

                if let Some(progress) = progress {
                    progress.finish().await;
                }
            }));
        }

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .roots
            .is_empty()
        {
            self.schedule();
        }
    }

    fn shutdown(&self) -> impl Future<Output = Result<()>> + Send {
        self.diagnostics.stop();

        if let Some(publication) = self
            .publication
            .lock()
            .expect("diagnostic publication")
            .take()
        {
            publication.abort();
        }

        self.shutdown.store(true, Ordering::Relaxed);
        self.workers.stop();
        self.queries.stop();

        ready(Ok(()))
    }
}

pub(crate) fn failure(error: &io::Error) -> Error {
    let code = match error.kind() {
        io::ErrorKind::Interrupted => ErrorCode::RequestCancelled,
        io::ErrorKind::WouldBlock => ErrorCode::ServerError(-32802),
        _ => ErrorCode::ServerError(-32803),
    };

    Error {
        code,
        message: error.to_string().into(),
        data: None,
    }
}
