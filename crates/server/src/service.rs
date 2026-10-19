use std::{
    collections::{BTreeSet, HashMap},
    future::Future,
    ops::ControlFlow,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use instar_bridge::Frontend;
use serde_json::{Value, json};
use tokio::sync;

use tower_lsp_server::{
    Client, LanguageServer,
    gen_lsp_types::{
        CodeActionParams, CodeActionResponse, ColorInformation, ColorPresentation,
        ColorPresentationParams, CompletionItem, CompletionParams, CompletionResponse,
        ConfigurationItem, CreateFilesParams, DefinitionParams, DefinitionResponse,
        DeleteFilesParams, DidChangeConfigurationParams, DidChangeTextDocumentParams,
        DidChangeWatchedFilesParams, DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams,
        DidOpenTextDocumentParams, DidSaveTextDocumentParams, DocumentColorParams,
        DocumentFormattingParams, DocumentHighlight, DocumentHighlightParams, DocumentLink,
        DocumentLinkParams, DocumentRangeFormattingParams, DocumentSymbolParams,
        DocumentSymbolResponse, ExecuteCommandParams, FoldingRange, FoldingRangeParams, Hover,
        HoverParams, InitializeParams, InitializeResult, InitializedParams, InlayHint,
        InlayHintParams, Location, MessageActionItem, MessageType, PrepareRenameResult,
        ProgressToken, Range, ReferenceParams, Registration, RenameFilesParams, RenameParams,
        SelectionRange, SelectionRangeParams, SemanticTokens, SemanticTokensDeltaParams,
        SemanticTokensDeltaResponse, SemanticTokensParams, SemanticTokensRangeParams,
        ShowDocumentParams, SignatureHelp, SignatureHelpParams, TextDocumentPositionParams,
        TextEdit, TypeDefinitionParams, TypeDefinitionResponse, Unregistration, Uri,
        WorkDoneProgressCancelParams, WorkspaceEdit, WorkspaceSymbolParams,
        WorkspaceSymbolResponse,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use super::{
    configuration::Configuration,
    connection::{Connection, Operation},
    diagnostics,
    document::{self, Encoding},
    editing, files, navigation, presentation,
    progress::{Progress, Registry},
    project::{Project, failure, resolved_uri},
    suggestions, synchronization,
};

pub(super) struct Service {
    client: Client,
    connection: Arc<Connection>,
    shutdown: Arc<AtomicBool>,
    capabilities: Mutex<Arc<Value>>,
    registrations: sync::Mutex<Registrations>,
    formatting: Arc<sync::Semaphore>,
    progress: Registry,
    sequence: AtomicU64,
    configuration_sequence: AtomicU64,
    configuration_revision: Arc<Mutex<u64>>,
}

#[derive(Default)]
struct Registrations {
    installed: Vec<Registration>,
    stopping: bool,
}

impl Registrations {
    async fn unregister(&mut self, client: &Client) -> Result<()> {
        if !self.installed.is_empty() {
            client
                .unregister_capability(
                    self.installed
                        .iter()
                        .map(|registration| Unregistration {
                            id: registration.id.clone(),
                            method: registration.method.clone(),
                        })
                        .collect(),
                )
                .await?;

            self.installed.clear();
        }

        Ok(())
    }
}

impl Service {
    pub(super) fn new(
        client: Client,
        connection: Arc<Connection>,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        Self {
            client,
            connection,
            shutdown,
            capabilities: Mutex::new(Arc::new(Value::Null)),
            registrations: sync::Mutex::new(Registrations::default()),
            formatting: Arc::new(sync::Semaphore::new(1)),
            progress: Arc::new(Mutex::new(HashMap::new())),
            sequence: AtomicU64::new(0),
            configuration_sequence: AtomicU64::new(0),
            configuration_revision: Arc::new(Mutex::new(0)),
        }
    }

    pub(super) fn cancel_progress(&self, parameters: &WorkDoneProgressCancelParams) {
        if let Some(operation) = self
            .progress
            .lock()
            .expect("internal error: progress registry poisoned")
            .get(&parameters.token)
            .and_then(Weak::upgrade)
        {
            operation.cancel();
        }
    }

    fn supports(&self, pointer: &str) -> bool {
        self.capabilities
            .lock()
            .expect("internal error: client capabilities poisoned")
            .pointer(pointer)
            .and_then(Value::as_bool)
            == Some(true)
    }

    async fn perform<Output, Pending>(
        &self,
        token: Option<ProgressToken>,
        execute: impl FnOnce(Arc<Operation>) -> Pending + Send,
    ) -> Result<Output>
    where
        Output: Send,
        Pending: Future<Output = Result<Output>> + Send,
    {
        let operation = Arc::new(Operation::new(
            token.clone().map(|token| (self.client.clone(), token)),
        ));

        let progress = Progress::new(
            self.client.clone(),
            token,
            &operation,
            Arc::clone(&self.progress),
        );

        progress.begin().await;
        let result = execute(operation).await;
        progress.finish().await;

        result
    }

    async fn query<Parameters, Output>(
        &self,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, &mut Frontend<'_>, Parameters) -> Result<Output>
        + Send
        + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(token, |operation| {
            self.connection
                .query(uri, operation, move |project, frontend| {
                    execute(project, frontend, parameters)
                })
        })
        .await
    }

    async fn source<Parameters, Output>(
        &self,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, Parameters) -> Result<Output> + Send + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(token, |operation| {
            self.connection
                .source(uri, operation, move |project| execute(project, parameters))
        })
        .await
    }

    async fn source_or_query<Parameters, Output>(
        &self,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        source: impl FnOnce(&mut Project, &Parameters) -> Result<ControlFlow<Output>> + Send + 'static,
        query: impl FnOnce(&mut Project, &mut Frontend<'_>, Parameters) -> Result<Output>
        + Send
        + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(token, |operation| async move {
            let prepared = self
                .connection
                .source(
                    uri.clone(),
                    Arc::clone(&operation),
                    move |project| match source(project, &parameters)? {
                        ControlFlow::Break(output) => Ok(ControlFlow::Break(output)),

                        ControlFlow::Continue(()) => Ok(ControlFlow::Continue((
                            parameters,
                            project.workspace.directory().to_owned(),
                            project.generation,
                            project.invalidation,
                        ))),
                    },
                )
                .await?;

            let (parameters, root, generation, invalidation) = match prepared {
                ControlFlow::Break(output) => return Ok(output),
                ControlFlow::Continue(prepared) => prepared,
            };

            self.connection
                .query(uri, operation, move |project, frontend| {
                    if project.workspace.directory() != root
                        || project.generation != generation
                        || project.invalidation != invalidation
                    {
                        return Err(Error::content_modified());
                    }

                    query(project, frontend, parameters)
                })
                .await
        })
        .await
    }

    async fn format(
        &self,
        uri: Uri,
        token: Option<ProgressToken>,
        range: Option<Range>,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.perform(token, |operation| async move {
            let permit = Arc::clone(&self.formatting)
                .acquire_owned()
                .await
                .map_err(failure)?;

            self.connection
                .formatting(uri, operation, permit, range)
                .await
        })
        .await
    }

    async fn notify<Parameters>(
        &self,
        uri: Uri,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, Parameters) -> Result<()> + Send + 'static,
    ) where
        Parameters: Send + 'static,
    {
        let result = async {
            self.connection
                .update(uri.clone(), move |project| execute(project, parameters))
                .await?;

            self.publish(uri, false).await
        }
        .await;

        if let Err(error) = result {
            self.report(error).await;
        }
    }

    async fn publish(&self, uri: Uri, progress: bool) -> Result<()> {
        let token = if progress && self.supports("/window/workDoneProgress") {
            let token = ProgressToken::String(format!(
                "instar/{}",
                self.sequence.fetch_add(1, Ordering::Relaxed)
            ));

            self.client.create_work_done_progress(token.clone()).await?;

            Some(token)
        } else {
            None
        };

        let publications = self
            .perform(token, |operation| {
                self.connection.query(uri, operation, |project, frontend| {
                    diagnostics::collect(project, frontend)
                })
            })
            .await?;

        for publication in publications {
            self.client
                .publish_diagnostics(
                    publication.uri,
                    publication.diagnostics,
                    publication.version,
                )
                .await;
        }

        Ok(())
    }

    async fn revalidate(&self, progress: bool) -> Result<()> {
        let roots = self.connection.control(|state| state.roots()).await?;

        for root in roots {
            if let Err(error) = self.publish(root, progress).await {
                self.report(error).await;
            }
        }

        Ok(())
    }

    async fn refresh(&self) {
        for (pointer, result) in [
            ("/workspace/semanticTokens/refreshSupport", 0),
            ("/workspace/inlayHint/refreshSupport", 1),
            ("/workspace/foldingRange/refreshSupport", 2),
        ] {
            if self.supports(pointer) {
                let response = match result {
                    0 => self.client.semantic_tokens_refresh().await,
                    1 => self.client.inlay_hint_refresh().await,
                    _ => self.client.folding_range_refresh().await,
                };

                if let Err(error) = response {
                    self.client
                        .log_message(MessageType::Warning, error.to_string())
                        .await;
                }
            }
        }
    }

    async fn report(&self, error: Error) {
        if matches!(
            error.code,
            ErrorCode::RequestCancelled | ErrorCode::ContentModified
        ) {
            return;
        }

        self.client
            .log_message(MessageType::Error, error.to_string())
            .await;

        self.client
            .show_message(MessageType::Error, error.message.into_owned())
            .await;
    }

    async fn configuration(&self, settings: Option<Value>) -> Result<bool> {
        let settings = settings
            .map(|settings| settings.get("instar").cloned().unwrap_or(settings))
            .filter(|settings| !settings.is_null())
            .map(Configuration::load)
            .transpose()?;

        let sequence = self
            .configuration_sequence
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |sequence| {
                sequence.checked_add(1)
            })
            .map_err(|_sequence| Error::internal_error())?;

        let roots = self.connection.control(|state| state.roots()).await?;

        let configurations = if self.supports("/workspace/configuration") {
            let items = if roots.is_empty() {
                vec![ConfigurationItem {
                    scope_uri: None,
                    section: Some("instar".to_owned()),
                }]
            } else {
                roots
                    .iter()
                    .map(|uri| ConfigurationItem {
                        scope_uri: Some(uri.clone()),
                        section: Some("instar".to_owned()),
                    })
                    .collect()
            };

            let expected = items.len();
            let values = self.client.configuration(items).await?;

            if values.len() != expected {
                return Err(Error::invalid_params(
                    "client returned an incorrect number of configurations",
                ));
            }

            values
                .into_iter()
                .map(|value| {
                    if value.is_null() {
                        Ok(None)
                    } else {
                        Configuration::load(value).map(Some)
                    }
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            vec![None; roots.len().max(1)]
        };

        if settings.is_none() && configurations.iter().all(Option::is_none) {
            return Ok(false);
        }

        let revision = Arc::clone(&self.configuration_revision);

        self.connection
            .control(move |state| {
                let mut revision = revision
                    .lock()
                    .expect("internal error: configuration revision poisoned");

                if sequence < *revision {
                    return Ok(false);
                }

                let paths = roots
                    .iter()
                    .map(document::path)
                    .collect::<Result<Vec<_>>>()?;

                if paths.iter().any(|path| !state.projects.contains_key(path)) {
                    return Err(Error::content_modified());
                }

                let mut changed = false;

                let fallback = if roots.is_empty() {
                    configurations
                        .first()
                        .and_then(Option::as_ref)
                        .or(settings.as_ref())
                } else {
                    settings.as_ref()
                };

                if let Some(configuration) = fallback
                    && serde_json::to_value(&state.configuration).map_err(failure)?
                        != serde_json::to_value(configuration).map_err(failure)?
                {
                    state.configuration = configuration.clone();
                    changed = true;
                }

                for (path, configuration) in paths.into_iter().zip(configurations) {
                    let Some(configuration) = configuration.or_else(|| settings.clone()) else {
                        continue;
                    };

                    let project = state
                        .projects
                        .get_mut(&path)
                        .ok_or_else(Error::content_modified)?;

                    if serde_json::to_value(&project.configuration).map_err(failure)?
                        != serde_json::to_value(&configuration).map_err(failure)?
                    {
                        project.changed()?;
                        project.configuration = configuration;
                        changed = true;
                    }
                }

                *revision = sequence;

                Ok(changed)
            })
            .await
    }

    async fn register(&self) -> Result<()> {
        let mut acknowledged = self.registrations.lock().await;

        if acknowledged.stopping {
            return Ok(());
        }

        let mut registrations = Vec::new();

        if self.supports("/workspace/didChangeWatchedFiles/dynamicRegistration") {
            let dependencies = self
                .connection
                .control(|state| {
                    Ok(state
                        .projects
                        .values()
                        .flat_map(Project::dependencies)
                        .collect::<BTreeSet<_>>())
                })
                .await?;

            let mut watchers = vec![json!({"globPattern":"**/*.{lua,luau,toml,json}"})];

            watchers.extend(dependencies.into_iter().filter_map(|path| {
                path.to_str()
                    .map(|path| json!({"globPattern":globset::escape(&path.replace('\\', "/"))}))
            }));

            registrations.push(Registration {
                id: "instar.files".to_owned(),
                method: "workspace/didChangeWatchedFiles".to_owned(),
                register_options: Some(json!({"watchers":watchers})),
            });
        }

        if self.supports("/workspace/didChangeConfiguration/dynamicRegistration") {
            registrations.push(Registration {
                id: "instar.configuration".to_owned(),
                method: "workspace/didChangeConfiguration".to_owned(),
                register_options: None,
            });
        }

        if registrations == acknowledged.installed {
            return Ok(());
        }

        acknowledged.unregister(&self.client).await?;

        if !registrations.is_empty() {
            self.client
                .register_capability(registrations.clone())
                .await?;

            acknowledged.installed = registrations;
        }

        Ok(())
    }

    async fn maintain(&self, progress: bool) -> Result<()> {
        self.register().await?;
        self.refresh().await;
        self.revalidate(progress).await?;

        Ok(())
    }

    async fn changed(&self, paths: Vec<PathBuf>) -> Result<()> {
        let changed = self
            .connection
            .control(move |state| files::changed(state, &paths))
            .await?;

        if !changed {
            return Ok(());
        }

        self.maintain(false).await
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
                            Error::invalid_params("workspace folder URI is missing")
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
            .expect("internal error: client capabilities poisoned") = Arc::clone(&capabilities);

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
                "documentLinkProvider":{},
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
            self.client
                .log_message(MessageType::Info, "Instar initialized")
                .await;

            if self.supports("/workspace/workspaceFolders")
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
            self.report(error).await;
        }
    }

    async fn shutdown(&self) -> Result<()> {
        let mut registrations = self.registrations.lock().await;
        registrations.stopping = true;

        self.connection
            .control(|state| {
                state.projects.clear();

                Ok(())
            })
            .await?;

        if let Err(error) = registrations.unregister(&self.client).await {
            self.client
                .log_message(MessageType::Warning, error.to_string())
                .await;
        }

        self.shutdown.store(true, Ordering::Relaxed);

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

        self.query(uri, None, parameters, suggestions::completion_resolve)
            .await
    }

    async fn signature_help(
        &self,
        parameters: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        self.query(
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
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| editing::code_action(project, &parameters),
        )
        .await
    }

    async fn inlay_hint(&self, parameters: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        self.source_or_query(
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
        self.query(
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, frontend, parameters| {
                navigation::document_link(project, frontend, &parameters)
            },
        )
        .await
    }

    async fn prepare_rename(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResult>> {
        self.source_or_query(
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
            parameters.text_document.uri.clone(),
            parameters.work_done_progress_params.work_done_token.clone(),
            parameters,
            |project, parameters| presentation::color_presentation(project, &parameters),
        )
        .await
    }

    async fn references(&self, parameters: ReferenceParams) -> Result<Option<Vec<Location>>> {
        self.source_or_query(
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

        self.perform(token, |operation| async move {
            let roots = self.connection.control(|state| state.roots()).await?;
            let mut symbols = Vec::new();
            let mut information = Vec::new();

            for root in roots {
                let parameters = parameters.clone();

                let result = self
                    .connection
                    .source(root, Arc::clone(&operation), move |project| {
                        navigation::symbol(project, &parameters)
                    })
                    .await?;

                match result {
                    Some(WorkspaceSymbolResponse::WorkspaceSymbolList(values)) => {
                        symbols.extend(values);
                    }

                    Some(WorkspaceSymbolResponse::SymbolInformationList(values)) => {
                        information.extend(values);
                    }

                    None => {}
                }
            }

            if symbols.is_empty() {
                Ok(Some(WorkspaceSymbolResponse::SymbolInformationList(
                    information,
                )))
            } else {
                Ok(Some(WorkspaceSymbolResponse::WorkspaceSymbolList(symbols)))
            }
        })
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
            self.report(error).await;
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
            self.report(error).await;
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
            self.report(error).await;
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
            self.report(error).await;
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
            self.report(error).await;
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
            self.report(error).await;
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
                .perform(None, |operation| {
                    self.connection
                        .query(root, operation, move |project, frontend| {
                            files::will_rename(project, frontend, parameters)
                        })
                })
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
            if !self.supports("/window/showDocument/support") {
                return Err(Error::invalid_params(
                    "client does not support opening documents",
                ));
            }

            let roots = self.connection.control(|state| state.roots()).await?;

            let actions = roots
                .iter()
                .map(|root| MessageActionItem {
                    title: root.to_string(),
                    ..MessageActionItem::default()
                })
                .collect();

            if let Some(action) = self
                .client
                .show_message_request(
                    MessageType::Info,
                    "Open a workspace's instar.toml",
                    Some(actions),
                )
                .await?
            {
                let root = roots
                    .into_iter()
                    .find(|root| root.as_str() == action.title)
                    .ok_or_else(|| Error::invalid_params("client selected an unknown workspace"))?;

                let uri = document::uri(&document::path(&root)?.join("instar.toml"))?;

                let opened = self
                    .client
                    .show_document(ShowDocumentParams::new(uri, None, Some(true), None))
                    .await?;

                return Ok(Some(Value::Bool(opened)));
            }

            return Ok(None);
        }

        let kind = match parameters.command.as_str() {
            "instar.fixAll" => "source.fixAll.instar",
            "instar.organizeImports" => "source.organizeImports",
            _ => return Err(Error::invalid_params("unknown Instar command")),
        };

        if !self.supports("/workspace/applyEdit") {
            return Err(Error::invalid_params(
                "client does not support workspace edits",
            ));
        }

        let arguments = parameters.arguments.unwrap_or_default();

        let [argument] = arguments.as_slice() else {
            return Err(Error::invalid_params("command expects one document URI"));
        };

        let uri: Uri = serde_json::from_value(argument.clone())
            .map_err(|error| Error::invalid_params(error.to_string()))?;

        let target = uri.clone();

        let edit = self
            .perform(
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
            return Err(failure(
                result
                    .failure_reason
                    .unwrap_or_else(|| "client declined workspace edit".to_owned()),
            ));
        }

        Ok(Some(Value::Bool(true)))
    }
}
