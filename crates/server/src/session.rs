use std::{
    collections::BTreeSet,
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
use tokio::time;

use tower_lsp_server::{
    Client, LanguageServer,
    gen_lsp_types::{
        DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, DocumentFormattingParams, InitializeParams, InitializeResult,
        InitializedParams, MessageType, Position, PositionEncodingKind, Range, Registration,
        ServerCapabilities, ServerInfo, TextDocumentSyncKind, TextDocumentSyncOptions, TextEdit,
        WorkspaceFolders, WorkspaceFoldersServerCapabilities, WorkspaceOptions,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use crate::{
    position,
    workers::Workers,
    workspace::{self, Workspace},
};

pub(crate) struct Session {
    client: Client,
    pub(crate) workspace: Mutex<Workspace>,
    pub(crate) workers: Arc<Workers>,
    options: Options,
    watching: AtomicBool,
    pub(crate) shutdown: Arc<AtomicBool>,
}

impl Session {
    pub(crate) fn new(
        client: Client,
        workers: Arc<Workers>,
        options: Options,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            workspace: Mutex::new(Workspace::default()),
            workers,
            options,
            watching: AtomicBool::new(false),
            shutdown,
        }
    }

    pub(crate) async fn open(&self, parameters: DidOpenTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .open(parameters);

        self.report(result).await;
    }

    pub(crate) async fn change(&self, parameters: DidChangeTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .change(parameters);

        self.report(result).await;
    }

    pub(crate) async fn close(&self, parameters: DidCloseTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .close(parameters);

        self.report(result).await;
    }

    pub(crate) async fn save(&self, parameters: DidSaveTextDocumentParams) {
        let result = self
            .workspace
            .lock()
            .expect("workspace lock")
            .save(parameters);

        self.report(result).await;
    }

    pub(crate) fn watched(&self, parameters: DidChangeWatchedFilesParams) -> Ready<()> {
        for change in parameters.changes {
            let result = workspace::path(&change.uri).and_then(|path| {
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

        ready(())
    }

    pub(crate) fn configuration(&self, _: DidChangeConfigurationParams) -> Ready<()> {
        self.workspace.lock().expect("workspace lock").invalidate();

        ready(())
    }

    pub(crate) async fn format(
        &self,
        parameters: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let (snapshot, path, encoding) = self
            .workspace
            .lock()
            .expect("workspace lock")
            .capture(&parameters.text_document.uri)?;

        let options = Options {
            cancellation: self.options.cancellation.child(),
            ..self.options.clone()
        };

        let timeout = options.timeout;

        let (_guard, response) = self
            .workers
            .submit(snapshot.clone(), path, options)
            .map_err(|error| failure(&error))?;

        let output = time::timeout(timeout, response)
            .await
            .map_err(|_| {
                failure(&io::Error::new(
                    io::ErrorKind::TimedOut,
                    "formatting deadline expired",
                ))
            })?
            .map_err(|_| failure(&io::Error::other("formatting worker stopped")))?;

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .current(&snapshot, &output.inputs)
        {
            return Err(Error::content_modified());
        }

        let Some((source, text)) = output.text.map_err(|error| failure(&error))? else {
            return Ok(None);
        };

        if source.as_ref() == text {
            return Ok(None);
        }

        Ok(Some(vec![TextEdit {
            range: Range {
                start: Position::default(),
                end: position::end(&source, &encoding),
            },
            new_text: text,
        }]))
    }

    async fn report(&self, result: Result<()>) {
        if let Err(error) = result {
            self.client
                .log_message(MessageType::Error, error.to_string())
                .await;
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
        let mut workspace = self.workspace.lock().expect("workspace lock");
        workspace.roots = roots;
        workspace.encoding = encoding.clone();

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
                document_formatting_provider: Some(true.into()),
                workspace: Some(WorkspaceOptions {
                    workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                        supported: Some(true),
                        change_notifications: Some(true.into()),
                    }),
                    ..WorkspaceOptions::default()
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
    }

    fn shutdown(&self) -> impl Future<Output = Result<()>> + Send {
        self.shutdown.store(true, Ordering::Relaxed);
        self.workers.stop();

        ready(Ok(()))
    }
}

fn failure(error: &io::Error) -> Error {
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
