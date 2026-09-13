mod actions;
mod capabilities;
mod color;
mod diagnostics;
mod documentation;
mod formatting;
mod hierarchy;
mod hints;
mod imports;
mod index;
mod lint;
mod progress;
mod refactor;
mod renames;
mod state;
mod structure;
mod sync;
mod workspace;

use std::{
    collections::BTreeMap,
    fmt::Display,
    io,
    path::PathBuf,
    process::ExitCode,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use crate::analysis;
use state::State;
use tokio::sync::oneshot;
use tower_lsp_server::ls_types as protocol;

use tower_lsp_server::{
    Client, LanguageServer, LspService, Server,
    jsonrpc::{Error, Result},
    ls_types::{
        DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
        DocumentFormattingParams, InitializeParams, InitializeResult, MessageType, OneOf, Position,
        PublishDiagnosticsParams, Range, ServerCapabilities, TextDocumentSyncCapability,
        TextDocumentSyncKind, TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Uri,
    },
};

enum Request {
    Open(DidOpenTextDocumentParams),
    Change(DidChangeTextDocumentParams),
    Close(DidCloseTextDocumentParams),
    Refresh,
    Format(DocumentFormattingParams),
    Range(protocol::DocumentRangeFormattingParams),
    Diagnostic(Uri),
    Configure(serde_json::Value),
    Hints(protocol::InlayHintParams),
    Actions(protocol::CodeActionParams),
    Query(protocol::TextDocumentPositionParams, &'static str),
    Workspace(Vec<Uri>, Vec<Uri>),
    Symbols(String),
    Complete(protocol::TextDocumentPositionParams),
    Resolve(Box<protocol::CompletionItem>),
    Hierarchy(protocol::CallHierarchyItem, bool),
    Prepare(protocol::TextDocumentPositionParams),
    Rename(protocol::RenameParams),
    Move(protocol::RenameFilesParams),
    Moving(protocol::RenameFilesParams),
}

enum Response {
    Diagnostics(Vec<PublishDiagnosticsParams>),
    Edits(Option<Vec<TextEdit>>),
    Editor(Vec<EditorEntry>),
    Rename(protocol::WorkspaceEdit),
    Actions(protocol::CodeActionResponse),
    Completions(Vec<protocol::CompletionItem>),
    Completion(Box<protocol::CompletionItem>),
    Hints(Vec<protocol::InlayHint>),
    Prepared(Vec<protocol::CallHierarchyItem>),
    Incoming(Vec<protocol::CallHierarchyIncomingCall>),
    Outgoing(Vec<protocol::CallHierarchyOutgoingCall>),
}

impl Request {
    fn changes(&self) -> bool {
        matches!(
            self,
            Self::Open(_)
                | Self::Change(_)
                | Self::Close(_)
                | Self::Refresh
                | Self::Move(_)
                | Self::Workspace(_, _)
                | Self::Configure(_)
        )
    }
}

#[derive(Clone)]
struct EditorEntry {
    native: analysis::EditorEntry,
    location: Option<protocol::Location>,
    selection: Option<Range>,
    documentation: Option<String>,
}

struct Message {
    request: Request,
    reply: oneshot::Sender<Result<Response>>,
    progress: tokio::sync::mpsc::UnboundedSender<(usize, usize)>,
}

fn symbol_kind(kind: Option<u32>) -> protocol::SymbolKind {
    match kind {
        Some(7) => protocol::SymbolKind::PROPERTY,
        Some(12) => protocol::SymbolKind::FUNCTION,
        Some(26) => protocol::SymbolKind::INTERFACE,
        _ => protocol::SymbolKind::VARIABLE,
    }
}

fn identifier(name: &str) -> bool {
    let mut characters = name.chars();

    characters
        .next()
        .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        && characters.all(|character| character == '_' || character.is_ascii_alphanumeric())
        && ![
            "and", "break", "continue", "do", "else", "elseif", "end", "false", "for", "function",
            "if", "in", "local", "nil", "not", "or", "repeat", "return", "then", "true", "until",
            "while", "const",
        ]
        .contains(&name)
}

fn internal_error(error: impl Display) -> Error {
    Error {
        message: error.to_string().into(),
        ..Error::internal_error()
    }
}

fn path(uri: &Uri) -> Result<PathBuf> {
    url::Url::parse(uri.as_str())
        .map_err(|error| Error::invalid_params(error.to_string()))?
        .to_file_path()
        .map_err(|()| Error::invalid_params("expected an absolute file URI"))
}

fn moved(uri: &Uri, old: &std::path::Path, new: &Uri) -> Result<Option<Uri>> {
    let original = path(uri)?;

    let Ok(suffix) = original.strip_prefix(old) else {
        return Ok(None);
    };

    if suffix.as_os_str().is_empty() {
        return Ok(Some(new.clone()));
    }

    Uri::from_file_path(path(new)?.join(suffix))
        .map(Some)
        .ok_or_else(|| internal_error("invalid renamed URI"))
}

struct Backend {
    client: Client,
    sender: mpsc::Sender<Message>,
    shutdown: Arc<AtomicBool>,
    registrations: std::sync::Mutex<Vec<protocol::Registration>>,
    versions: std::sync::Mutex<BTreeMap<Uri, i32>>,
    generation: AtomicU64,
    refresh_tokens: AtomicBool,
    progress: AtomicBool,
    progress_identifier: AtomicU64,
    refresh_hints: AtomicBool,
    diagnostics: std::sync::Mutex<diagnostics::Cache>,
}

impl Backend {
    async fn request(&self, request: Request) -> Result<Response> {
        let generation = matches!(
            request,
            Request::Query(_, _)
                | Request::Format(_)
                | Request::Range(_)
                | Request::Diagnostic(_)
                | Request::Hints(_)
                | Request::Rename(_)
                | Request::Moving(_)
                | Request::Symbols(_)
                | Request::Actions(_)
                | Request::Complete(_)
                | Request::Resolve(_)
                | Request::Hierarchy(_, _)
                | Request::Prepare(_)
        )
        .then(|| self.generation.load(Ordering::Relaxed));

        let (reply, receiver) = oneshot::channel();

        let (progress, events) = tokio::sync::mpsc::unbounded_channel();

        let token = self.progress.load(Ordering::Relaxed).then(|| {
            protocol::ProgressToken::String(format!(
                "index-{}",
                self.progress_identifier.fetch_add(1, Ordering::Relaxed)
            ))
        });

        self.sender
            .send(Message {
                request,
                reply,
                progress,
            })
            .map_err(internal_error)?;

        let response = progress::wait(receiver, events, &self.client, token).await;

        if generation
            .is_some_and(|generation| generation != self.generation.load(Ordering::Relaxed))
        {
            return Err(Error::content_modified());
        }

        response
    }

    async fn query(
        &self,
        parameters: protocol::TextDocumentPositionParams,
        operation: &'static str,
    ) -> Result<Vec<EditorEntry>> {
        match self.request(Request::Query(parameters, operation)).await? {
            Response::Editor(entries) => Ok(entries),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn notify(&self, request: Request) {
        let generation = self
            .generation
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_add(1);

        {
            let mut versions = self.versions.lock().expect("document versions");

            match &request {
                Request::Open(parameters) => {
                    versions.insert(
                        parameters.text_document.uri.clone(),
                        parameters.text_document.version,
                    );
                }

                Request::Change(parameters) => {
                    versions
                        .entry(parameters.text_document.uri.clone())
                        .and_modify(|version| {
                            *version = (*version).max(parameters.text_document.version);
                        });
                }

                Request::Close(parameters) => {
                    versions.remove(&parameters.text_document.uri);
                }

                Request::Move(parameters) => {
                    for file in &parameters.files {
                        if let (Some(old), Ok(new)) = (
                            file.old_uri
                                .parse::<Uri>()
                                .ok()
                                .and_then(|uri| path(&uri).ok()),
                            file.new_uri.parse::<Uri>(),
                        ) {
                            let moves = versions
                                .iter()
                                .filter_map(|(uri, version)| {
                                    moved(uri, &old, &new)
                                        .ok()
                                        .flatten()
                                        .map(|new| (uri.clone(), new, *version))
                                })
                                .collect::<Vec<_>>();

                            for (old, new, version) in moves {
                                versions.remove(&old);
                                versions.entry(new).or_insert(version);
                            }
                        }
                    }
                }

                _ => {}
            }
        }

        match self.request(request).await {
            Ok(Response::Diagnostics(publications)) => {
                let refresh =
                    !publications.is_empty() && self.refresh_tokens.load(Ordering::Relaxed);

                for publication in publications {
                    if generation != self.generation.load(Ordering::Relaxed) {
                        return;
                    }

                    let current = self
                        .versions
                        .lock()
                        .expect("document versions")
                        .get(&publication.uri)
                        .copied();

                    if publication.version != current {
                        continue;
                    }

                    self.client
                        .publish_diagnostics(
                            publication.uri,
                            publication.diagnostics,
                            publication.version,
                        )
                        .await;
                }

                if refresh {
                    drop(self.client.semantic_tokens_refresh().await);
                }
            }

            Ok(
                Response::Edits(_)
                | Response::Editor(_)
                | Response::Rename(_)
                | Response::Actions(_)
                | Response::Completions(_)
                | Response::Completion(_)
                | Response::Hints(_)
                | Response::Prepared(_)
                | Response::Incoming(_)
                | Response::Outgoing(_),
            ) => {}

            Err(error) => {
                self.client
                    .log_message(MessageType::ERROR, error.to_string())
                    .await;
            }
        }
    }
}

impl LanguageServer for Backend {
    async fn initialize(&self, parameters: InitializeParams) -> Result<InitializeResult> {
        self.progress.store(
            parameters
                .capabilities
                .window
                .as_ref()
                .and_then(|window| window.work_done_progress)
                .unwrap_or_default(),
            Ordering::Relaxed,
        );

        self.refresh_hints.store(
            parameters
                .capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.inlay_hint.as_ref())
                .and_then(|hints| hints.refresh_support)
                .unwrap_or_default(),
            Ordering::Relaxed,
        );

        if let Some(options) = parameters.initialization_options {
            self.request(Request::Configure(options)).await?;
        }

        self.refresh_tokens.store(
            parameters
                .capabilities
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.semantic_tokens.as_ref())
                .and_then(|tokens| tokens.refresh_support)
                .unwrap_or_default(),
            Ordering::Relaxed,
        );

        if let Some(workspace) = parameters.capabilities.workspace {
            let mut registrations = self.registrations.lock().map_err(internal_error)?;

            if workspace
                .did_change_watched_files
                .and_then(|capabilities| capabilities.dynamic_registration)
                == Some(true)
            {
                registrations.push(protocol::Registration {
                    id: "files".into(),
                    method: "workspace/didChangeWatchedFiles".into(),
                    register_options: Some(
                        r#"{"watchers":[{"globPattern":"**/*"}]}"#
                            .parse()
                            .map_err(internal_error)?,
                    ),
                });
            }

            if workspace
                .did_change_configuration
                .and_then(|capabilities| capabilities.dynamic_registration)
                == Some(true)
            {
                registrations.push(protocol::Registration {
                    id: "configuration".into(),
                    method: "workspace/didChangeConfiguration".into(),
                    register_options: None,
                });
            }
        }

        #[expect(
            deprecated,
            reason = "Clients without workspace folders still supply the legacy root URI"
        )]
        let roots = parameters.workspace_folders.map_or_else(
            || parameters.root_uri.into_iter().collect(),
            |folders| folders.into_iter().map(|folder| folder.uri).collect(),
        );

        self.request(Request::Workspace(roots, Vec::new())).await?;

        let operations = protocol::FileOperationRegistrationOptions {
            filters: vec![protocol::FileOperationFilter {
                scheme: Some("file".into()),
                pattern: protocol::FileOperationPattern {
                    glob: "**/*".into(),
                    ..protocol::FileOperationPattern::default()
                },
            }],
        };

        Ok(InitializeResult {
            capabilities: capabilities::server(operations),
            ..InitializeResult::default()
        })
    }

    async fn initialized(&self, _: protocol::InitializedParams) {
        let registrations = self
            .registrations
            .lock()
            .map(|mut registrations| std::mem::take(&mut *registrations))
            .map_err(|_| ());

        let Ok(registrations) = registrations else {
            return;
        };

        if !registrations.is_empty()
            && let Err(error) = self.client.register_capability(registrations).await
        {
            self.client
                .log_message(MessageType::ERROR, error.to_string())
                .await;
        }
    }

    async fn did_change_workspace_folders(
        &self,
        parameters: protocol::DidChangeWorkspaceFoldersParams,
    ) {
        self.notify(Request::Workspace(
            parameters
                .event
                .added
                .into_iter()
                .map(|folder| folder.uri)
                .collect(),
            parameters
                .event
                .removed
                .into_iter()
                .map(|folder| folder.uri)
                .collect(),
        ))
        .await;

        self.notify(Request::Refresh).await;
    }

    async fn symbol(
        &self,
        parameters: protocol::WorkspaceSymbolParams,
    ) -> Result<Option<protocol::WorkspaceSymbolResponse>> {
        let Response::Editor(entries) = self.request(Request::Symbols(parameters.query)).await?
        else {
            return Err(internal_error("unexpected worker response"));
        };

        #[expect(
            deprecated,
            reason = "The protocol symbol type still requires its deprecated compatibility field"
        )]
        let symbols = entries
            .into_iter()
            .filter_map(|entry| {
                Some(protocol::SymbolInformation {
                    name: entry.native.name?,
                    kind: symbol_kind(entry.native.kind),
                    tags: None,
                    deprecated: None,
                    location: entry.location?,
                    container_name: None,
                })
            })
            .collect();

        Ok(Some(protocol::WorkspaceSymbolResponse::Flat(symbols)))
    }

    fn shutdown(&self) -> impl Future<Output = Result<()>> + Send {
        self.shutdown.store(true, Ordering::Relaxed);

        std::future::ready(Ok(()))
    }

    async fn did_open(&self, parameters: DidOpenTextDocumentParams) {
        self.notify(Request::Open(parameters)).await;
    }

    async fn did_change(&self, parameters: DidChangeTextDocumentParams) {
        self.notify(Request::Change(parameters)).await;
    }

    async fn did_close(&self, parameters: DidCloseTextDocumentParams) {
        self.notify(Request::Close(parameters)).await;
    }

    async fn did_create_files(&self, _: protocol::CreateFilesParams) {
        self.notify(Request::Refresh).await;
    }

    async fn will_rename_files(
        &self,
        parameters: protocol::RenameFilesParams,
    ) -> Result<Option<protocol::WorkspaceEdit>> {
        match self.request(Request::Moving(parameters)).await? {
            Response::Rename(edit) => Ok(Some(edit)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn did_rename_files(&self, parameters: protocol::RenameFilesParams) {
        self.notify(Request::Move(parameters)).await;
    }

    async fn did_delete_files(&self, _: protocol::DeleteFilesParams) {
        self.notify(Request::Refresh).await;
    }

    async fn did_save(&self, _: DidSaveTextDocumentParams) {
        self.notify(Request::Refresh).await;
    }

    async fn did_change_watched_files(&self, _: DidChangeWatchedFilesParams) {
        self.notify(Request::Refresh).await;
    }

    async fn did_change_configuration(&self, parameters: DidChangeConfigurationParams) {
        self.notify(Request::Configure(parameters.settings)).await;

        if self.refresh_hints.load(Ordering::Relaxed) {
            drop(self.client.inlay_hint_refresh().await);
        }
    }

    async fn inlay_hint(
        &self,
        parameters: protocol::InlayHintParams,
    ) -> Result<Option<Vec<protocol::InlayHint>>> {
        match self.request(Request::Hints(parameters)).await? {
            Response::Hints(hints) => Ok(Some(hints)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn range_formatting(
        &self,
        parameters: protocol::DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        match self.request(Request::Range(parameters)).await? {
            Response::Edits(edits) => Ok(edits),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn diagnostic(
        &self,
        parameters: protocol::DocumentDiagnosticParams,
    ) -> Result<protocol::DocumentDiagnosticReportResult> {
        let uri = parameters.text_document.uri;

        let Response::Diagnostics(publications) =
            self.request(Request::Diagnostic(uri.clone())).await?
        else {
            return Err(internal_error("unexpected worker response"));
        };

        let requested = path(&uri)?;

        let items = publications
            .into_iter()
            .find(|publication| path(&publication.uri).is_ok_and(|path| path == requested))
            .map(|publication| publication.diagnostics)
            .unwrap_or_default();

        Ok(self
            .diagnostics
            .lock()
            .map_err(internal_error)?
            .report(&requested, items, parameters.previous_result_id.as_deref())
            .into())
    }

    async fn hover(&self, parameters: protocol::HoverParams) -> Result<Option<protocol::Hover>> {
        let Some(entry) = self
            .query(parameters.text_document_position_params, "hover")
            .await?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };

        let Some(description) = entry.native.description else {
            return Ok(None);
        };

        let mut value = format!("```luau\n{description}\n```");

        if let Some(documentation) = entry.documentation {
            value.push_str("\n\n");
            value.push_str(&documentation);
        }

        Ok(Some(protocol::Hover {
            contents: protocol::HoverContents::Markup(protocol::MarkupContent {
                kind: protocol::MarkupKind::Markdown,
                value,
            }),
            range: entry.location.map(|location| location.range),
        }))
    }

    async fn goto_definition(
        &self,
        parameters: protocol::GotoDefinitionParams,
    ) -> Result<Option<protocol::GotoDefinitionResponse>> {
        Ok(self
            .query(parameters.text_document_position_params, "definition")
            .await?
            .into_iter()
            .find_map(|entry| entry.location)
            .map(protocol::GotoDefinitionResponse::Scalar))
    }

    async fn goto_implementation(
        &self,
        parameters: protocol::request::GotoImplementationParams,
    ) -> Result<Option<protocol::request::GotoImplementationResponse>> {
        let locations = self
            .query(parameters.text_document_position_params, "implementation")
            .await?
            .into_iter()
            .filter_map(|entry| entry.location)
            .map(|location| {
                (
                    (
                        location.uri.to_string(),
                        location.range.start,
                        location.range.end,
                    ),
                    location,
                )
            })
            .collect::<BTreeMap<_, _>>()
            .into_values()
            .collect();

        Ok(Some(protocol::GotoDefinitionResponse::Array(locations)))
    }

    async fn goto_declaration(
        &self,
        parameters: protocol::request::GotoDeclarationParams,
    ) -> Result<Option<protocol::request::GotoDeclarationResponse>> {
        self.goto_definition(parameters).await
    }

    async fn goto_type_definition(
        &self,
        parameters: protocol::request::GotoTypeDefinitionParams,
    ) -> Result<Option<protocol::request::GotoTypeDefinitionResponse>> {
        Ok(self
            .query(parameters.text_document_position_params, "typeDefinition")
            .await?
            .into_iter()
            .find_map(|entry| entry.location)
            .map(protocol::GotoDefinitionResponse::Scalar))
    }

    async fn references(
        &self,
        parameters: protocol::ReferenceParams,
    ) -> Result<Option<Vec<protocol::Location>>> {
        Ok(Some(
            self.query(parameters.text_document_position, "references")
                .await?
                .into_iter()
                .filter(|entry| {
                    parameters.context.include_declaration || entry.native.declaration != Some(true)
                })
                .filter_map(|entry| entry.location)
                .map(|location| {
                    (
                        (
                            location.uri.clone(),
                            location.range.start,
                            location.range.end,
                        ),
                        location,
                    )
                })
                .collect::<BTreeMap<_, _>>()
                .into_values()
                .collect(),
        ))
    }

    async fn prepare_rename(
        &self,
        parameters: protocol::TextDocumentPositionParams,
    ) -> Result<Option<protocol::PrepareRenameResponse>> {
        let entries = self.query(parameters.clone(), "prepare").await?;

        if !entries
            .iter()
            .any(|entry| entry.native.declaration == Some(true))
        {
            return Ok(None);
        }

        Ok(entries.into_iter().find_map(|entry| {
            let location = entry.location?;
            let range = entry.selection.unwrap_or(location.range);

            (location.uri == parameters.text_document.uri
                && range.start <= parameters.position
                && parameters.position <= range.end)
                .then(|| protocol::PrepareRenameResponse::RangeWithPlaceholder {
                    range,
                    placeholder: entry.native.name.unwrap_or_default(),
                })
        }))
    }

    async fn rename(
        &self,
        parameters: protocol::RenameParams,
    ) -> Result<Option<protocol::WorkspaceEdit>> {
        match self.request(Request::Rename(parameters)).await? {
            Response::Rename(edit) => Ok(Some(edit)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn completion(
        &self,
        parameters: protocol::CompletionParams,
    ) -> Result<Option<protocol::CompletionResponse>> {
        match self
            .request(Request::Complete(parameters.text_document_position))
            .await?
        {
            Response::Completions(items) => Ok(Some(protocol::CompletionResponse::Array(items))),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn completion_resolve(
        &self,
        item: protocol::CompletionItem,
    ) -> Result<protocol::CompletionItem> {
        match self.request(Request::Resolve(Box::new(item))).await? {
            Response::Completion(item) => Ok(*item),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn prepare_call_hierarchy(
        &self,
        parameters: protocol::CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<protocol::CallHierarchyItem>>> {
        match self
            .request(Request::Prepare(parameters.text_document_position_params))
            .await?
        {
            Response::Prepared(items) => Ok(Some(items)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn incoming_calls(
        &self,
        parameters: protocol::CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<protocol::CallHierarchyIncomingCall>>> {
        match self
            .request(Request::Hierarchy(parameters.item, true))
            .await?
        {
            Response::Incoming(items) => Ok(Some(items)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn outgoing_calls(
        &self,
        parameters: protocol::CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<protocol::CallHierarchyOutgoingCall>>> {
        match self
            .request(Request::Hierarchy(parameters.item, false))
            .await?
        {
            Response::Outgoing(items) => Ok(Some(items)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn signature_help(
        &self,
        parameters: protocol::SignatureHelpParams,
    ) -> Result<Option<protocol::SignatureHelp>> {
        let entries = self
            .query(parameters.text_document_position_params, "signature")
            .await?;

        let signatures = entries
            .into_iter()
            .filter_map(|entry| {
                Some(protocol::SignatureInformation {
                    label: entry.native.label?,
                    documentation: entry.documentation.map(|value| {
                        protocol::Documentation::MarkupContent(protocol::MarkupContent {
                            kind: protocol::MarkupKind::Markdown,
                            value,
                        })
                    }),
                    parameters: entry.native.parameters.map(|parameters| {
                        parameters
                            .into_iter()
                            .map(|label| protocol::ParameterInformation {
                                label: protocol::ParameterLabel::Simple(label),
                                documentation: None,
                            })
                            .collect()
                    }),
                    active_parameter: entry.native.active,
                })
            })
            .collect::<Vec<_>>();

        if signatures.is_empty() {
            return Ok(None);
        }

        Ok(Some(protocol::SignatureHelp {
            signatures,
            active_signature: None,
            active_parameter: None,
        }))
    }

    async fn document_color(
        &self,
        parameters: protocol::DocumentColorParams,
    ) -> Result<Vec<protocol::ColorInformation>> {
        color::colors(self, parameters).await
    }

    fn color_presentation(
        &self,
        parameters: protocol::ColorPresentationParams,
    ) -> impl Future<Output = Result<Vec<protocol::ColorPresentation>>> {
        std::future::ready(color::presentations(&parameters))
    }

    async fn document_highlight(
        &self,
        parameters: protocol::DocumentHighlightParams,
    ) -> Result<Option<Vec<protocol::DocumentHighlight>>> {
        structure::highlights(self, parameters).await
    }

    async fn folding_range(
        &self,
        parameters: protocol::FoldingRangeParams,
    ) -> Result<Option<Vec<protocol::FoldingRange>>> {
        structure::folds(self, parameters).await
    }

    async fn selection_range(
        &self,
        parameters: protocol::SelectionRangeParams,
    ) -> Result<Option<Vec<protocol::SelectionRange>>> {
        structure::selections(self, parameters).await
    }

    async fn document_symbol(
        &self,
        parameters: protocol::DocumentSymbolParams,
    ) -> Result<Option<protocol::DocumentSymbolResponse>> {
        let entries = self
            .query(
                protocol::TextDocumentPositionParams {
                    text_document: parameters.text_document,
                    position: Position::default(),
                },
                "symbols",
            )
            .await?;

        #[expect(
            deprecated,
            reason = "The protocol document symbol type still requires its deprecated compatibility field"
        )]
        let symbols = entries
            .into_iter()
            .filter_map(|entry| {
                Some(protocol::DocumentSymbol {
                    name: entry.native.name?,
                    detail: entry.native.description,
                    kind: symbol_kind(entry.native.kind),
                    tags: None,
                    deprecated: None,
                    range: entry.location?.range,
                    selection_range: entry.selection?,
                    children: None,
                })
            })
            .collect();

        Ok(Some(protocol::DocumentSymbolResponse::Nested(symbols)))
    }

    async fn document_link(
        &self,
        parameters: protocol::DocumentLinkParams,
    ) -> Result<Option<Vec<protocol::DocumentLink>>> {
        let entries = self
            .query(
                protocol::TextDocumentPositionParams {
                    text_document: parameters.text_document,
                    position: Position::default(),
                },
                "links",
            )
            .await?;

        Ok(Some(
            entries
                .into_iter()
                .filter_map(|entry| {
                    let location = entry.location?;
                    let target = Uri::from_file_path(PathBuf::from(entry.native.name?))?;

                    Some(protocol::DocumentLink {
                        range: location.range,
                        target: Some(target),
                        tooltip: None,
                        data: None,
                    })
                })
                .collect(),
        ))
    }

    async fn semantic_tokens_full(
        &self,
        parameters: protocol::SemanticTokensParams,
    ) -> Result<Option<protocol::SemanticTokensResult>> {
        let mut entries = self
            .query(
                protocol::TextDocumentPositionParams {
                    text_document: parameters.text_document,
                    position: Position::default(),
                },
                "tokens",
            )
            .await?;

        entries.sort_by_key(|entry| {
            entry
                .location
                .as_ref()
                .map(|location| (location.range.start.line, location.range.start.character))
        });

        let mut previous = Position::default();
        let mut data = Vec::new();

        for entry in entries {
            let Some(location) = entry.location else {
                continue;
            };

            let range = location.range;

            if range.start.line != range.end.line || range.start.character == range.end.character {
                continue;
            }

            data.push(protocol::SemanticToken {
                delta_line: range.start.line - previous.line,
                delta_start: if range.start.line == previous.line {
                    range.start.character - previous.character
                } else {
                    range.start.character
                },
                length: range.end.character - range.start.character,
                token_type: match entry.native.kind {
                    Some(12) => 1,
                    Some(7) => 2,
                    Some(26) => 3,
                    Some(3) => 4,
                    Some(27) => 5,
                    Some(6) => 6,
                    Some(28) => 7,
                    Some(5) => 8,
                    _ => 0,
                },
                token_modifiers_bitset: u32::from(entry.native.declaration == Some(true))
                    | entry.native.modifiers.unwrap_or_default(),
            });

            previous = range.start;
        }

        Ok(Some(protocol::SemanticTokensResult::Tokens(
            protocol::SemanticTokens {
                result_id: None,
                data,
            },
        )))
    }

    async fn code_action(
        &self,
        parameters: protocol::CodeActionParams,
    ) -> Result<Option<protocol::CodeActionResponse>> {
        match self.request(Request::Actions(parameters)).await? {
            Response::Actions(actions) => Ok(Some(actions)),
            _ => Err(internal_error("unexpected worker response")),
        }
    }

    async fn formatting(
        &self,
        parameters: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        match self.request(Request::Format(parameters)).await? {
            Response::Edits(edits) => Ok(edits),
            _ => Err(internal_error("unexpected worker response")),
        }
    }
}

/// # Errors
/// Returns runtime, transport, or worker failures.
pub fn run() -> io::Result<ExitCode> {
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let shutdown = Arc::<AtomicBool>::default();
    let (sender, receiver) = mpsc::channel::<Message>();

    let worker = thread::Builder::new().spawn(move || {
        let mut state = State::default();

        let mut replies = Vec::new();

        while let Ok(first) = receiver.recv() {
            let messages = std::iter::once(first)
                .chain(receiver.try_iter())
                .collect::<Vec<_>>();

            for message in messages {
                if message.reply.is_closed() {
                    continue;
                }

                if message.request.changes() {
                    state.progress = Some(message.progress);

                    match state.handle(message.request) {
                        Ok(_) => replies.push(message.reply),

                        Err(error) => {
                            drop(message.reply.send(Err(error)));
                        }
                    }
                } else {
                    state.publish(&mut replies);
                    state.progress = Some(message.progress);
                    drop(message.reply.send(state.handle(message.request)));
                }
            }

            state.publish(&mut replies);
        }
    })?;

    runtime.block_on(async {
        let (service, socket) = LspService::new(|client| Backend {
            client,
            sender,
            shutdown: Arc::clone(&shutdown),
            registrations: std::sync::Mutex::default(),
            versions: std::sync::Mutex::default(),
            generation: AtomicU64::default(),
            refresh_tokens: AtomicBool::default(),
            progress: AtomicBool::default(),
            progress_identifier: AtomicU64::default(),
            refresh_hints: AtomicBool::default(),
            diagnostics: std::sync::Mutex::default(),
        });

        Server::new(tokio::io::stdin(), tokio::io::stdout(), socket)
            .serve(service)
            .await;
    });

    worker
        .join()
        .map_err(|_| io::Error::other("language server worker panicked"))?;

    Ok(if shutdown.load(Ordering::Relaxed) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}
