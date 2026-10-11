use serde_json::{Value, json};

use super::super::support::{Fixture, Session};

pub(super) fn capabilities() -> Value {
    json!({
        "general":{"positionEncodings":["utf-16"]},
        "textDocument":{
            "synchronization":{"willSave":true,"willSaveWaitUntil":true,"didSave":true},
            "hover":{"contentFormat":["markdown","plaintext"]},
            "completion":{"completionItem":{"snippetSupport":true,"documentationFormat":["markdown"],"resolveSupport":{"properties":["documentation","detail","additionalTextEdits"]}}},
            "inlineCompletion":{},
            "signatureHelp":{"signatureInformation":{"parameterInformation":{"labelOffsetSupport":true},"activeParameterSupport":true}},
            "diagnostic":{"relatedDocumentSupport":true},
            "publishDiagnostics":{"relatedInformation":true,"versionSupport":true,"tagSupport":{"valueSet":[1,2]}},
            "documentSymbol":{"hierarchicalDocumentSymbolSupport":true},
            "rename":{"prepareSupport":true},
            "codeAction":{"codeActionLiteralSupport":{"codeActionKind":{"valueSet":["quickfix","source.fixAll.instar","source.organizeImports","refactor.extract"]}},"resolveSupport":{"properties":["edit"]}},
            "codeLens":{},
            "inlayHint":{"resolveSupport":{"properties":["tooltip"]}},
            "inlineValue":{},
            "callHierarchy":{},
            "typeHierarchy":{},
            "documentLink":{"tooltipSupport":true},
            "foldingRange":{"lineFoldingOnly":true},
            "selectionRange":{},
            "linkedEditingRange":{},
            "colorProvider":{},
            "semanticTokens":{"requests":{"range":true,"full":{"delta":true}},"tokenTypes":["comment","keyword","string","number","operator","decorator","type","typeParameter","variable","parameter","function","property","method"],"tokenModifiers":["declaration","modification","defaultLibrary","deprecated"],"formats":["relative"]}
        },
        "workspace":{
            "applyEdit":true,
            "workspaceEdit":{"documentChanges":true,"resourceOperations":["create","rename","delete"],"failureHandling":"abort","changeAnnotationSupport":{"groupsOnLabel":true}},
            "symbol":{"resolveSupport":{"properties":["location.range"]}},
            "fileOperations":{"didCreate":true,"willCreate":true,"didRename":true,"willRename":true,"didDelete":true,"willDelete":true},
            "textDocumentContent":{"refreshSupport":true}
        },
        "experimental":{"rangesFormatting":true}
    })
}

pub(super) fn connect(fixture: &Fixture, capabilities: &Value) -> Session {
    let mut session = Session::new(fixture);
    session.reply(
        "workspace/workspaceFolders",
        json!([{"uri":fixture.uri(""),"name":"contract"}]),
    );
    let initialized = session.request(
        "initialize",
        &json!({
            "processId":null,
            "rootUri":fixture.uri(""),
            "workspaceFolders":[{"uri":fixture.uri(""),"name":"contract"}],
            "capabilities":capabilities
        }),
    );
    assert_eq!(initialized["serverInfo"]["name"], "Instar");
    session.notify("initialized", &json!({}));
    session
}

pub(super) fn open(session: &mut Session, fixture: &Fixture, name: &str) -> String {
    let text = fixture.read(name);
    let address = fixture.uri(name);
    session.notify(
        "textDocument/didOpen",
        &json!({"textDocument":{"uri":address,"languageId":"luau","version":1,"text":text}}),
    );
    assert_eq!(
        session.request("workspace/textDocumentContent", &json!({"uri":address}))["text"],
        text
    );
    text
}
