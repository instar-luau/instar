use std::future::{Future, ready};

use instar_project::PositionQueryKind;

use tower_lsp_server::{
    LanguageServer,
    gen_lsp_types::{
        CallHierarchyIncomingCall, CallHierarchyIncomingCallsParams, CallHierarchyItem,
        CallHierarchyOutgoingCall, CallHierarchyOutgoingCallsParams, CallHierarchyPrepareParams,
        CodeAction, CodeActionParams, CodeActionResponse, CodeLens, CodeLensParams,
        ColorInformation, ColorPresentation, ColorPresentationParams, CompletionItem,
        CompletionParams, CompletionResponse, CreateFilesParams, Declaration, DeclarationParams,
        DeclarationResponse, Definition, DefinitionParams, DefinitionResponse, DeleteFilesParams,
        DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
        DidChangeWorkspaceFoldersParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
        DidSaveTextDocumentParams, DocumentColorParams, DocumentDiagnosticParams,
        DocumentDiagnosticReport, DocumentFormattingParams, DocumentHighlight,
        DocumentHighlightParams, DocumentLink, DocumentLinkParams, DocumentOnTypeFormattingParams,
        DocumentRangeFormattingParams, DocumentRangesFormattingParams, DocumentSymbolParams,
        DocumentSymbolResponse, ExecuteCommandParams, FoldingRange, FoldingRangeParams, Hover,
        HoverParams, ImplementationParams, ImplementationResponse, InitializeParams,
        InitializeResult, InitializedParams, InlayHint, InlayHintParams, InlineCompletionParams,
        InlineCompletionResponse, InlineValue, InlineValueParams, LinkedEditingRangeParams,
        LinkedEditingRanges, Location as ProtocolLocation, LspAny, PrepareRenameResult,
        ReferenceParams, RenameFilesParams, RenameParams, SelectionRange, SelectionRangeParams,
        SemanticTokens, SemanticTokensDeltaParams, SemanticTokensDeltaResponse,
        SemanticTokensParams, SemanticTokensRangeParams, SignatureHelp, SignatureHelpParams,
        TextDocumentContentParams, TextDocumentContentResult, TextDocumentPositionParams,
        TextEdit as ProtocolTextEdit, TypeDefinitionParams, TypeDefinitionResponse,
        TypeHierarchyItem, TypeHierarchyPrepareParams, TypeHierarchySubtypesParams,
        TypeHierarchySupertypesParams, WillSaveTextDocumentParams, WorkspaceDiagnosticParams,
        WorkspaceDiagnosticReport, WorkspaceEdit, WorkspaceSymbol, WorkspaceSymbolParams,
        WorkspaceSymbolResponse,
    },
    jsonrpc::Result,
};

use super::{
    assistance, diagnostics, editing, formatting, hierarchy, navigation, presentation,
    synchronization, workspace,
};
use crate::Server;

impl LanguageServer for Server {
    fn initialize(
        &self,
        parameters: InitializeParams,
    ) -> impl Future<Output = Result<InitializeResult>> + Send {
        ready(Self::initialize(self, parameters))
    }

    async fn initialized(&self, parameters: InitializedParams) {
        Self::initialized(self, parameters).await;
    }

    async fn shutdown(&self) -> Result<()> {
        Self::shutdown(self).await
    }

    async fn did_open(&self, parameters: DidOpenTextDocumentParams) {
        synchronization::open(self, parameters).await;
    }

    async fn did_change(&self, parameters: DidChangeTextDocumentParams) {
        synchronization::change(self, parameters).await;
    }

    async fn did_close(&self, parameters: DidCloseTextDocumentParams) {
        synchronization::close(self, parameters).await;
    }

    async fn did_save(&self, parameters: DidSaveTextDocumentParams) {
        synchronization::save(self, parameters).await;
    }

    async fn will_save(&self, parameters: WillSaveTextDocumentParams) {
        synchronization::will_save(self, parameters).await;
    }

    async fn will_save_wait_until(
        &self,
        parameters: WillSaveTextDocumentParams,
    ) -> Result<Option<Vec<ProtocolTextEdit>>> {
        synchronization::save_edits(self, parameters).await
    }

    async fn diagnostic(
        &self,
        parameters: DocumentDiagnosticParams,
    ) -> Result<DocumentDiagnosticReport> {
        diagnostics::document(self, parameters).await
    }

    async fn workspace_diagnostic(
        &self,
        parameters: WorkspaceDiagnosticParams,
    ) -> Result<WorkspaceDiagnosticReport> {
        diagnostics::workspace(self, parameters).await
    }

    async fn hover(&self, parameters: HoverParams) -> Result<Option<Hover>> {
        assistance::hover(self, parameters).await
    }

    async fn completion(&self, parameters: CompletionParams) -> Result<Option<CompletionResponse>> {
        assistance::completion(self, parameters).await
    }

    async fn completion_resolve(&self, parameters: CompletionItem) -> Result<CompletionItem> {
        assistance::completion_resolve(self, parameters).await
    }

    async fn inline_completion(
        &self,
        parameters: InlineCompletionParams,
    ) -> Result<Option<InlineCompletionResponse>> {
        assistance::inline_completion(self, parameters).await
    }

    async fn signature_help(
        &self,
        parameters: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        assistance::signatures(self, parameters).await
    }

    async fn code_lens(&self, parameters: CodeLensParams) -> Result<Option<Vec<CodeLens>>> {
        assistance::lenses(self, parameters).await
    }

    fn code_lens_resolve(
        &self,
        parameters: CodeLens,
    ) -> impl Future<Output = Result<CodeLens>> + Send {
        ready(assistance::lens_resolve(self, parameters))
    }

    async fn inlay_hint(&self, parameters: InlayHintParams) -> Result<Option<Vec<InlayHint>>> {
        assistance::hints(self, parameters).await
    }

    async fn inlay_hint_resolve(&self, parameters: InlayHint) -> Result<InlayHint> {
        assistance::hint_resolve(self, parameters).await
    }

    async fn inline_value(
        &self,
        parameters: InlineValueParams,
    ) -> Result<Option<Vec<InlineValue>>> {
        assistance::inline_values(self, parameters).await
    }

    async fn goto_declaration(
        &self,
        parameters: DeclarationParams,
    ) -> Result<Option<DeclarationResponse>> {
        navigation::navigate(
            self,
            parameters.text_document_position_params,
            PositionQueryKind::Declaration,
            parameters.work_done_progress_params.work_done_token,
        )
        .await
        .map(|locations| {
            Some(DeclarationResponse::Declaration(Declaration::LocationList(
                locations,
            )))
        })
    }

    async fn goto_definition(
        &self,
        parameters: DefinitionParams,
    ) -> Result<Option<DefinitionResponse>> {
        navigation::navigate(
            self,
            parameters.text_document_position_params,
            PositionQueryKind::Definition,
            parameters.work_done_progress_params.work_done_token,
        )
        .await
        .map(|locations| {
            Some(DefinitionResponse::Definition(Definition::LocationList(
                locations,
            )))
        })
    }

    async fn goto_type_definition(
        &self,
        parameters: TypeDefinitionParams,
    ) -> Result<Option<TypeDefinitionResponse>> {
        navigation::navigate(
            self,
            parameters.text_document_position_params,
            PositionQueryKind::TypeDefinition,
            parameters.work_done_progress_params.work_done_token,
        )
        .await
        .map(|locations| {
            Some(TypeDefinitionResponse::Definition(
                Definition::LocationList(locations),
            ))
        })
    }

    async fn goto_implementation(
        &self,
        parameters: ImplementationParams,
    ) -> Result<Option<ImplementationResponse>> {
        navigation::navigate(
            self,
            parameters.text_document_position_params,
            PositionQueryKind::Implementation,
            parameters.work_done_progress_params.work_done_token,
        )
        .await
        .map(|locations| {
            Some(ImplementationResponse::Definition(
                Definition::LocationList(locations),
            ))
        })
    }

    async fn references(
        &self,
        parameters: ReferenceParams,
    ) -> Result<Option<Vec<ProtocolLocation>>> {
        navigation::references(self, parameters).await
    }

    async fn document_highlight(
        &self,
        parameters: DocumentHighlightParams,
    ) -> Result<Option<Vec<DocumentHighlight>>> {
        navigation::highlights(self, parameters).await
    }

    async fn document_symbol(
        &self,
        parameters: DocumentSymbolParams,
    ) -> Result<Option<DocumentSymbolResponse>> {
        navigation::symbols(self, parameters).await
    }

    async fn symbol(
        &self,
        parameters: WorkspaceSymbolParams,
    ) -> Result<Option<WorkspaceSymbolResponse>> {
        navigation::search(self, parameters).await
    }

    fn symbol_resolve(
        &self,
        parameters: WorkspaceSymbol,
    ) -> impl Future<Output = Result<WorkspaceSymbol>> + Send {
        ready(navigation::symbol_resolve(self, parameters))
    }

    async fn document_link(
        &self,
        parameters: DocumentLinkParams,
    ) -> Result<Option<Vec<DocumentLink>>> {
        navigation::links(self, parameters).await
    }

    fn document_link_resolve(
        &self,
        parameters: DocumentLink,
    ) -> impl Future<Output = Result<DocumentLink>> + Send {
        ready(navigation::link_resolve(self, parameters))
    }

    async fn prepare_call_hierarchy(
        &self,
        parameters: CallHierarchyPrepareParams,
    ) -> Result<Option<Vec<CallHierarchyItem>>> {
        hierarchy::prepare_calls(self, parameters).await
    }

    async fn incoming_calls(
        &self,
        parameters: CallHierarchyIncomingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyIncomingCall>>> {
        hierarchy::incoming(self, parameters).await
    }

    async fn outgoing_calls(
        &self,
        parameters: CallHierarchyOutgoingCallsParams,
    ) -> Result<Option<Vec<CallHierarchyOutgoingCall>>> {
        hierarchy::outgoing(self, parameters).await
    }

    async fn prepare_type_hierarchy(
        &self,
        parameters: TypeHierarchyPrepareParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        hierarchy::prepare_types(self, parameters).await
    }

    async fn supertypes(
        &self,
        parameters: TypeHierarchySupertypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        hierarchy::supertypes(self, parameters).await
    }

    async fn subtypes(
        &self,
        parameters: TypeHierarchySubtypesParams,
    ) -> Result<Option<Vec<TypeHierarchyItem>>> {
        hierarchy::subtypes(self, parameters).await
    }

    async fn prepare_rename(
        &self,
        parameters: TextDocumentPositionParams,
    ) -> Result<Option<PrepareRenameResult>> {
        editing::prepare(self, parameters).await
    }

    async fn rename(&self, parameters: RenameParams) -> Result<Option<WorkspaceEdit>> {
        editing::rename(self, parameters).await
    }

    async fn code_action(
        &self,
        parameters: CodeActionParams,
    ) -> Result<Option<Vec<CodeActionResponse>>> {
        editing::actions(self, parameters).await
    }

    fn code_action_resolve(
        &self,
        parameters: CodeAction,
    ) -> impl Future<Output = Result<CodeAction>> + Send {
        ready(editing::resolve(self, parameters))
    }

    async fn linked_editing_range(
        &self,
        parameters: LinkedEditingRangeParams,
    ) -> Result<Option<LinkedEditingRanges>> {
        editing::linked(self, parameters).await
    }

    async fn formatting(
        &self,
        parameters: DocumentFormattingParams,
    ) -> Result<Option<Vec<ProtocolTextEdit>>> {
        formatting::whole(self, parameters).await
    }

    async fn range_formatting(
        &self,
        parameters: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<ProtocolTextEdit>>> {
        formatting::range(self, parameters).await
    }

    async fn ranges_formatting(
        &self,
        parameters: DocumentRangesFormattingParams,
    ) -> Result<Option<Vec<ProtocolTextEdit>>> {
        formatting::multiple(self, parameters).await
    }

    async fn on_type_formatting(
        &self,
        parameters: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<ProtocolTextEdit>>> {
        formatting::character(self, parameters).await
    }

    async fn semantic_tokens_full(
        &self,
        parameters: SemanticTokensParams,
    ) -> Result<Option<SemanticTokens>> {
        presentation::tokens(self, parameters).await
    }

    async fn semantic_tokens_full_delta(
        &self,
        parameters: SemanticTokensDeltaParams,
    ) -> Result<Option<SemanticTokensDeltaResponse>> {
        presentation::delta(self, parameters).await
    }

    async fn semantic_tokens_range(
        &self,
        parameters: SemanticTokensRangeParams,
    ) -> Result<Option<SemanticTokens>> {
        presentation::token_range(self, parameters).await
    }

    async fn folding_range(
        &self,
        parameters: FoldingRangeParams,
    ) -> Result<Option<Vec<FoldingRange>>> {
        presentation::folds(self, parameters).await
    }

    async fn selection_range(
        &self,
        parameters: SelectionRangeParams,
    ) -> Result<Option<Vec<SelectionRange>>> {
        presentation::selections(self, parameters).await
    }

    async fn document_color(
        &self,
        parameters: DocumentColorParams,
    ) -> Result<Vec<ColorInformation>> {
        presentation::colors(self, parameters).await
    }

    async fn color_presentation(
        &self,
        parameters: ColorPresentationParams,
    ) -> Result<Vec<ColorPresentation>> {
        presentation::color(self, parameters).await
    }

    async fn did_change_workspace_folders(&self, parameters: DidChangeWorkspaceFoldersParams) {
        workspace::folders(self, parameters).await;
    }

    async fn did_change_watched_files(&self, parameters: DidChangeWatchedFilesParams) {
        workspace::watched(self, parameters).await;
    }

    async fn did_change_configuration(&self, parameters: DidChangeConfigurationParams) {
        workspace::configuration(self, parameters).await;
    }

    async fn did_create_files(&self, parameters: CreateFilesParams) {
        workspace::created(self, parameters).await;
    }

    async fn did_rename_files(&self, parameters: RenameFilesParams) {
        workspace::renamed(self, parameters).await;
    }

    async fn did_delete_files(&self, parameters: DeleteFilesParams) {
        workspace::deleted(self, parameters).await;
    }

    async fn will_create_files(
        &self,
        parameters: CreateFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        workspace::will_create(self, parameters).await
    }

    async fn will_rename_files(
        &self,
        parameters: RenameFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        workspace::will_rename(self, parameters).await
    }

    async fn will_delete_files(
        &self,
        parameters: DeleteFilesParams,
    ) -> Result<Option<WorkspaceEdit>> {
        workspace::will_delete(self, parameters).await
    }

    async fn execute_command(&self, parameters: ExecuteCommandParams) -> Result<Option<LspAny>> {
        workspace::execute(self, parameters).await
    }

    async fn text_document_content(
        &self,
        parameters: TextDocumentContentParams,
    ) -> Result<TextDocumentContentResult> {
        workspace::content(self, parameters).await
    }
}
