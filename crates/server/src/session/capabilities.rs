use serde_json::{Value, json};

use tower_lsp_server::{
    gen_lsp_types::{
        ClientCapabilities, MarkupKind, PositionEncodingKind, SemanticTokensLegend,
        SemanticTokensOptions, ServerCapabilities,
    },
    jsonrpc::{Error, Result},
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
    values: Value,
    pub(crate) tokens: Option<SemanticTokensOptions>,
}

impl Capabilities {
    pub(crate) fn new(client: ClientCapabilities) -> Result<Self> {
        let values = serde_json::to_value(&client)
            .map_err(|error| Error::invalid_params(error.to_string()))?;

        let tokens = values.pointer("/textDocument/semanticTokens").filter(|tokens| tokens.get("formats").and_then(Value::as_array).is_some_and(|formats| formats.iter().any(|format| format == "relative"))).and_then(|tokens| {
            let supported = |property: &str, names: &[&str]| names.iter().filter(|name| tokens.get(property).and_then(Value::as_array).is_some_and(|values| values.iter().any(|value| value.as_str() == Some(**name)))).map(|name| (*name).to_owned()).collect::<Vec<_>>();
            let legend = SemanticTokensLegend { token_types: supported("tokenTypes", TOKEN_KINDS), token_modifiers: supported("tokenModifiers", TOKEN_MODIFIERS) };

            if legend.token_types.is_empty() || !tokens.pointer("/requests/full").is_some_and(|value| value == true || value.is_object()) && !tokens.pointer("/requests/range").is_some_and(|value| value == true || value.is_object()) { return None; }

            serde_json::from_value(json!({"legend":legend,"full":tokens.pointer("/requests/full"),"range":tokens.pointer("/requests/range")})).ok()
        });

        Ok(Self {
            client,
            values,
            tokens,
        })
    }

    pub(crate) fn supports(&self, pointer: &str) -> bool {
        self.values
            .pointer(pointer)
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    pub(crate) fn has(&self, pointer: &str) -> bool {
        self.values
            .pointer(pointer)
            .is_some_and(|value| !value.is_null() && value != false)
    }

    pub(crate) fn resolves(&self, operation: &str, property: &str) -> bool {
        self.values
            .pointer(&format!(
                "/textDocument/{operation}/resolveSupport/properties"
            ))
            .and_then(Value::as_array)
            .is_some_and(|properties| {
                properties
                    .iter()
                    .any(|value| value.as_str() == Some(property))
            })
    }

    pub(crate) fn markup(&self, operation: &str) -> MarkupKind {
        let pointer = match operation {
            "hover" => "/textDocument/hover/contentFormat",
            "completion" => "/textDocument/completion/completionItem/documentationFormat",
            "signatures" => "/textDocument/signatureHelp/signatureInformation/documentationFormat",
            _ => return MarkupKind::PlainText,
        };

        self.values
            .pointer(pointer)
            .and_then(Value::as_array)
            .and_then(|formats| {
                formats.iter().find_map(|format| match format.as_str() {
                    Some("markdown") => Some(MarkupKind::Markdown),
                    Some("plaintext") => Some(MarkupKind::PlainText),
                    _ => None,
                })
            })
            .unwrap_or(MarkupKind::PlainText)
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

    pub(crate) fn advertised(&self) -> Result<ServerCapabilities> {
        let files =
            json!({"filters":[{"scheme":"file","pattern":{"glob":"**/*","matches":"file"}}]});

        let mut capabilities = json!({
            "positionEncoding": self.encoding(),
            "textDocumentSync": {"openClose":true,"change":2,"save":true,"willSave":true,"willSaveWaitUntil":true},
            "diagnosticProvider":{"interFileDependencies":true,"workspaceDiagnostics":true},
            "documentFormattingProvider":true,"documentRangeFormattingProvider":true,
            "documentOnTypeFormattingProvider":{"firstTriggerCharacter":"\n","moreTriggerCharacter":[";"]},
            "hoverProvider":true,"completionProvider":{"resolveProvider":true,"triggerCharacters":[".",":","\"","'","/","@"]},
            "signatureHelpProvider":{"triggerCharacters":["(",","]},
            "declarationProvider":true,"definitionProvider":true,"typeDefinitionProvider":true,"implementationProvider":true,
            "referencesProvider":true,"documentHighlightProvider":true,"documentSymbolProvider":true,
            "workspaceSymbolProvider":if self.has("/workspace/symbol/resolveSupport") { json!({"resolveProvider":true}) } else { json!(true) },
            "renameProvider":if self.supports("/textDocument/rename/prepareSupport") { json!({"prepareProvider":true}) } else { json!(true) },
            "codeActionProvider":if self.has("/textDocument/codeAction/codeActionLiteralSupport") { json!({"resolveProvider":true,"codeActionKinds":["quickfix","source.fixAll.instar","source.organizeImports","refactor.extract"]}) } else { json!(true) },
            "codeLensProvider":{"resolveProvider":true},"documentLinkProvider":{"resolveProvider":true},
            "inlayHintProvider":if self.resolves("inlayHint", "tooltip") { json!({"resolveProvider":true}) } else { json!({}) },"inlineValueProvider":true,
            "callHierarchyProvider":true,"typeHierarchyProvider":true,"linkedEditingRangeProvider":true,
            "colorProvider":true,"foldingRangeProvider":true,"selectionRangeProvider":true,
            "executeCommandProvider":{"commands":COMMANDS},
            "workspace":{"workspaceFolders":{"supported":true,"changeNotifications":true},"fileOperations":{"didCreate":files,"willCreate":files,"didRename":files,"willRename":files,"didDelete":files,"willDelete":files}}
        });

        if let Some(tokens) = &self.tokens {
            capabilities["semanticTokensProvider"] =
                serde_json::to_value(tokens).map_err(|_| Error::internal_error())?;
        }

        if self.has("/textDocument/inlineCompletion") {
            capabilities["inlineCompletionProvider"] = json!({});
        }

        if self.has("/workspace/textDocumentContent") {
            capabilities["workspace"]["textDocumentContent"] =
                json!({"schemes":["instar","untitled"]});
        }

        if self.supports("/experimental/rangesFormatting") {
            capabilities["experimental"] = json!({"rangesFormatting":true});
        }

        serde_json::from_value(capabilities)
            .map_err(|error| Error::invalid_params(error.to_string()))
    }
}
