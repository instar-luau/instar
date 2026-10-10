use serde_json::{Value, json};

use tower_lsp_server::gen_lsp_types::{
    ClientCapabilities, ClientSemanticTokensRequestOptionsFull,
    ClientSemanticTokensRequestOptionsRange, CodeActionOptions, CodeLensOptions, CompletionOptions,
    DiagnosticOptions, DocumentLinkOptions, DocumentOnTypeFormattingOptions, ExecuteCommandOptions,
    FileOperationFilter, FileOperationOptions, FileOperationPattern, FileOperationPatternKind,
    FileOperationRegistrationOptions, Full, InlayHintOptions, InlineCompletionOptions, MarkupKind,
    PositionEncodingKind, RenameOptions, SemanticTokensFullDelta, SemanticTokensLegend,
    SemanticTokensOptions, SemanticTokensOptionsRange, ServerCapabilities, SignatureHelpOptions,
    TextDocumentClientCapabilities, TextDocumentContentOptions, TextDocumentSyncKind,
    TextDocumentSyncOptions, TokenFormat, WorkspaceClientCapabilities,
    WorkspaceFoldersServerCapabilities, WorkspaceOptions, WorkspaceSymbolOptions,
};

pub(crate) const COMMANDS: &[&str] = &[
    "instar.applyFix",
    "instar.fixAll",
    "instar.organizeImports",
    "instar.extractLiteral",
    "instar.check",
    "instar.lint",
    "instar.format",
    "instar.showSettings",
];

pub(crate) const TOKEN_KINDS: &[&str] = &[
    "comment",
    "keyword",
    "string",
    "number",
    "operator",
    "decorator",
    "type",
    "typeParameter",
    "variable",
    "parameter",
    "function",
    "property",
    "method",
];

pub(crate) const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "modification",
    "defaultLibrary",
    "deprecated",
];

#[derive(Default)]
pub(crate) struct Capabilities {
    pub(crate) client: ClientCapabilities,
    pub(crate) tokens: Option<SemanticTokensOptions>,
}

impl Capabilities {
    pub(crate) fn new(client: ClientCapabilities) -> Self {
        let tokens = client
            .text_document
            .as_ref()
            .and_then(|document| document.semantic_tokens.as_ref())
            .filter(|tokens| tokens.formats.contains(&TokenFormat::Relative))
            .and_then(|tokens| {
                let supported = |names: &[&str], values: &[String]| {
                    names.iter()
                        .filter(|name| values.iter().any(|value| value.as_str() == **name))
                        .map(|name| (*name).to_owned())
                        .collect::<Vec<_>>()
                };

                let legend = SemanticTokensLegend {
                    token_types: supported(TOKEN_KINDS, &tokens.token_types),
                    token_modifiers: supported(TOKEN_MODIFIERS, &tokens.token_modifiers),
                };

                let full = matches!(
                    tokens.requests.full.as_ref(),
                    Some(ClientSemanticTokensRequestOptionsFull::Bool(true)
                        | ClientSemanticTokensRequestOptionsFull::ClientSemanticTokensRequestFullDelta(_))
                );

                let range = matches!(
                    tokens.requests.range.as_ref(),
                    Some(ClientSemanticTokensRequestOptionsRange::Bool(true)
                        | ClientSemanticTokensRequestOptionsRange::Object(_))
                );

                if legend.token_types.is_empty() || !full && !range {
                    return None;
                }

                Some(SemanticTokensOptions {
                    legend,
                    full: tokens.requests.full.as_ref().map(|full| match full {
                        ClientSemanticTokensRequestOptionsFull::Bool(value) => (*value).into(),

                        ClientSemanticTokensRequestOptionsFull::ClientSemanticTokensRequestFullDelta(full) =>
                            SemanticTokensFullDelta { delta: full.delta }.into(),
                    }),
                    range: tokens.requests.range.as_ref().map(|range| match range {
                        ClientSemanticTokensRequestOptionsRange::Bool(value) => (*value).into(),
                        ClientSemanticTokensRequestOptionsRange::Object(value) => value.clone().into(),
                    }),
                    ..SemanticTokensOptions::default()
                })
            });

        Self { client, tokens }
    }

    pub(crate) fn text_document(&self) -> Option<&TextDocumentClientCapabilities> {
        self.client.text_document.as_ref()
    }

    pub(crate) fn workspace(&self) -> Option<&WorkspaceClientCapabilities> {
        self.client.workspace.as_ref()
    }

    pub(crate) fn hover_markup(&self) -> MarkupKind {
        preferred_markup(
            self.text_document()
                .and_then(|document| document.hover.as_ref())
                .and_then(|hover| hover.content_format.as_deref()),
        )
    }

    pub(crate) fn completion_markup(&self) -> MarkupKind {
        preferred_markup(
            self.text_document()
                .and_then(|document| document.completion.as_ref())
                .and_then(|completion| completion.completion_item.as_ref())
                .and_then(|item| item.documentation_format.as_deref()),
        )
    }

    pub(crate) fn signature_markup(&self) -> MarkupKind {
        preferred_markup(
            self.text_document()
                .and_then(|document| document.signature_help.as_ref())
                .and_then(|signature| signature.signature_information.as_ref())
                .and_then(|information| information.documentation_format.as_deref()),
        )
    }

    pub(crate) fn code_action_literals(&self) -> bool {
        self.text_document()
            .and_then(|document| document.code_action.as_ref())
            .is_some_and(|action| action.code_action_literal_support.is_some())
    }

    pub(crate) fn code_action_edits(&self) -> bool {
        self.text_document()
            .and_then(|document| document.code_action.as_ref())
            .and_then(|action| action.resolve_support.as_ref())
            .is_some_and(|support| support.properties.iter().any(|property| property == "edit"))
    }

    pub(crate) fn hint_tooltips(&self) -> bool {
        self.text_document()
            .and_then(|document| document.inlay_hint.as_ref())
            .and_then(|hint| hint.resolve_support.as_ref())
            .is_some_and(|support| {
                support
                    .properties
                    .iter()
                    .any(|property| property == "tooltip")
            })
    }

    pub(crate) fn full_tokens(&self) -> bool {
        matches!(
            self.tokens.as_ref().and_then(|tokens| tokens.full.as_ref()),
            Some(Full::Bool(true) | Full::SemanticTokensFullDelta(_))
        )
    }

    pub(crate) fn range_tokens(&self) -> bool {
        matches!(
            self.tokens
                .as_ref()
                .and_then(|tokens| tokens.range.as_ref()),
            Some(SemanticTokensOptionsRange::Bool(true) | SemanticTokensOptionsRange::Object(_))
        )
    }

    pub(crate) fn token_deltas(&self) -> bool {
        matches!(self.tokens.as_ref().and_then(|tokens| tokens.full.as_ref()),
            Some(Full::SemanticTokensFullDelta(full)) if full.delta == Some(true))
    }

    pub(crate) fn pull_diagnostics(&self) -> bool {
        self.text_document()
            .is_some_and(|document| document.diagnostic.is_some())
    }

    pub(crate) fn ranges_formatting(&self) -> bool {
        self.client
            .experimental
            .as_ref()
            .and_then(|values| values.get("rangesFormatting"))
            .and_then(Value::as_bool)
            == Some(true)
    }

    pub(crate) fn encoding(&self) -> PositionEncodingKind {
        self.client
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
            .unwrap_or(PositionEncodingKind::UTF16)
    }

    pub(crate) fn advertised(&self) -> ServerCapabilities {
        let files = FileOperationRegistrationOptions {
            filters: vec![FileOperationFilter {
                scheme: Some("file".to_owned()),
                pattern: FileOperationPattern::new(
                    "**/*".to_owned(),
                    Some(FileOperationPatternKind::File),
                    None,
                ),
            }],
        };

        let symbols = self
            .workspace()
            .and_then(|workspace| workspace.symbol.as_ref())
            .is_some_and(|symbol| symbol.resolve_support.is_some());

        let rename = self
            .text_document()
            .and_then(|document| document.rename.as_ref())
            .is_some_and(|rename| rename.prepare_support == Some(true));

        ServerCapabilities {
            position_encoding: Some(self.encoding()),
            text_document_sync: Some(
                TextDocumentSyncOptions {
                    open_close: Some(true),
                    change: Some(TextDocumentSyncKind::Incremental),
                    will_save: Some(true),
                    will_save_wait_until: Some(true),
                    save: Some(true.into()),
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
                resolve_provider: Some(true),
                trigger_characters: Some(
                    [".", ":", "\"", "'", "/", "@"].map(str::to_owned).to_vec(),
                ),
                ..CompletionOptions::default()
            }),
            signature_help_provider: Some(SignatureHelpOptions {
                trigger_characters: Some(["(", ","].map(str::to_owned).to_vec()),
                ..SignatureHelpOptions::default()
            }),
            declaration_provider: Some(true.into()),
            definition_provider: Some(true.into()),
            type_definition_provider: Some(true.into()),
            implementation_provider: Some(true.into()),
            references_provider: Some(true.into()),
            document_highlight_provider: Some(true.into()),
            document_symbol_provider: Some(true.into()),
            workspace_symbol_provider: Some(if symbols {
                WorkspaceSymbolOptions {
                    resolve_provider: Some(true),
                    ..WorkspaceSymbolOptions::default()
                }
                .into()
            } else {
                true.into()
            }),
            rename_provider: Some(if rename {
                RenameOptions {
                    prepare_provider: Some(true),
                    ..RenameOptions::default()
                }
                .into()
            } else {
                true.into()
            }),
            code_action_provider: Some(if self.code_action_literals() {
                CodeActionOptions {
                    resolve_provider: Some(true),
                    code_action_kinds: Some(
                        [
                            "quickfix",
                            "source.fixAll.instar",
                            "source.organizeImports",
                            "refactor.extract",
                        ]
                        .map(Into::into)
                        .to_vec(),
                    ),
                    ..CodeActionOptions::default()
                }
                .into()
            } else {
                true.into()
            }),
            code_lens_provider: Some(CodeLensOptions {
                resolve_provider: Some(true),
                ..CodeLensOptions::default()
            }),
            document_link_provider: Some(DocumentLinkOptions {
                resolve_provider: Some(true),
                ..DocumentLinkOptions::default()
            }),
            inlay_hint_provider: Some(
                InlayHintOptions {
                    resolve_provider: self.hint_tooltips().then_some(true),
                    ..InlayHintOptions::default()
                }
                .into(),
            ),
            inline_value_provider: Some(true.into()),
            call_hierarchy_provider: Some(true.into()),
            type_hierarchy_provider: Some(true.into()),
            linked_editing_range_provider: Some(true.into()),
            color_provider: Some(true.into()),
            folding_range_provider: Some(true.into()),
            selection_range_provider: Some(true.into()),
            execute_command_provider: Some(ExecuteCommandOptions {
                commands: COMMANDS
                    .iter()
                    .map(|command| (*command).to_owned())
                    .collect(),
                ..ExecuteCommandOptions::default()
            }),
            semantic_tokens_provider: self.tokens.clone().map(Into::into),
            inline_completion_provider: self
                .text_document()
                .filter(|document| document.inline_completion.is_some())
                .map(|_| InlineCompletionOptions::default().into()),
            workspace: Some(WorkspaceOptions {
                workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                    supported: Some(true),
                    change_notifications: Some(true.into()),
                }),
                file_operations: Some(FileOperationOptions {
                    did_create: Some(files.clone()),
                    will_create: Some(files.clone()),
                    did_rename: Some(files.clone()),
                    will_rename: Some(files.clone()),
                    did_delete: Some(files.clone()),
                    will_delete: Some(files),
                }),
                text_document_content: self
                    .workspace()
                    .filter(|workspace| workspace.text_document_content.is_some())
                    .map(|_| {
                        TextDocumentContentOptions {
                            schemes: vec!["instar".to_owned(), "untitled".to_owned()],
                        }
                        .into()
                    }),
            }),
            experimental: self
                .ranges_formatting()
                .then(|| json!({"rangesFormatting":true})),
            ..ServerCapabilities::default()
        }
    }
}

fn preferred_markup(formats: Option<&[MarkupKind]>) -> MarkupKind {
    formats
        .into_iter()
        .flatten()
        .find(|format| matches!(format, MarkupKind::Markdown | MarkupKind::PlainText))
        .cloned()
        .unwrap_or(MarkupKind::PlainText)
}
