use super::{
    OneOf, ServerCapabilities, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, protocol,
};

pub(super) fn server(operations: protocol::FileOperationRegistrationOptions) -> ServerCapabilities {
    ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..TextDocumentSyncOptions::default()
            },
        )),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_link_provider: Some(protocol::DocumentLinkOptions {
            resolve_provider: None,
            work_done_progress_options: protocol::WorkDoneProgressOptions::default(),
        }),
        hover_provider: Some(protocol::HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        declaration_provider: Some(protocol::DeclarationCapability::Simple(true)),
        type_definition_provider: Some(protocol::TypeDefinitionProviderCapability::Simple(true)),
        references_provider: Some(OneOf::Left(true)),
        call_hierarchy_provider: Some(protocol::CallHierarchyServerCapability::Simple(true)),
        completion_provider: Some(protocol::CompletionOptions {
            trigger_characters: Some(vec![".".into(), ":".into(), "\"".into(), "'".into()]),
            ..protocol::CompletionOptions::default()
        }),
        signature_help_provider: Some(protocol::SignatureHelpOptions {
            trigger_characters: Some(vec!["(".into(), ",".into()]),
            ..protocol::SignatureHelpOptions::default()
        }),
        document_symbol_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        color_provider: Some(protocol::ColorProviderCapability::Simple(true)),
        folding_range_provider: Some(protocol::FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(protocol::SelectionRangeProviderCapability::Simple(true)),
        semantic_tokens_provider: Some(
            protocol::SemanticTokensServerCapabilities::SemanticTokensOptions(
                protocol::SemanticTokensOptions {
                    legend: protocol::SemanticTokensLegend {
                        token_types: vec![
                            protocol::SemanticTokenType::VARIABLE,
                            protocol::SemanticTokenType::FUNCTION,
                            protocol::SemanticTokenType::PROPERTY,
                            protocol::SemanticTokenType::TYPE,
                            protocol::SemanticTokenType::NAMESPACE,
                            protocol::SemanticTokenType::PARAMETER,
                            protocol::SemanticTokenType::METHOD,
                            protocol::SemanticTokenType::TYPE_PARAMETER,
                            protocol::SemanticTokenType::CLASS,
                        ],
                        token_modifiers: vec![
                            protocol::SemanticTokenModifier::DECLARATION,
                            protocol::SemanticTokenModifier::READONLY,
                        ],
                    },
                    full: Some(protocol::SemanticTokensFullOptions::Bool(true)),
                    ..protocol::SemanticTokensOptions::default()
                },
            ),
        ),
        code_action_provider: Some(protocol::CodeActionProviderCapability::Simple(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(protocol::RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: protocol::WorkDoneProgressOptions::default(),
        })),
        workspace: Some(protocol::WorkspaceServerCapabilities {
            workspace_folders: Some(protocol::WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            file_operations: Some(protocol::WorkspaceFileOperationsServerCapabilities {
                did_create: Some(operations.clone()),
                did_rename: Some(operations.clone()),
                did_delete: Some(operations),
                ..protocol::WorkspaceFileOperationsServerCapabilities::default()
            }),
        }),
        ..ServerCapabilities::default()
    }
}
