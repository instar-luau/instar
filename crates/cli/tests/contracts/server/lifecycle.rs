use serde_json::{Value, json};

use super::{
    super::support::{Fixture, Session, apply, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn negotiation() {
    let fixture = Fixture::new("cli", "server");
    let mut session = Session::new(&fixture);
    let premature = session.response("workspace/symbol", json!({"query":"value"}));
    assert_eq!(premature["error"]["code"], -32002);
    let initialized = session.request("initialize", json!({"processId":null,"workspaceFolders":[{"uri":fixture.uri(""),"name":"contract"}],"capabilities":{}}));
    assert_eq!(initialized["capabilities"]["positionEncoding"], "utf-16");
    assert_eq!(initialized["capabilities"]["textDocumentSync"]["change"], 2);
    assert_eq!(
        initialized["capabilities"]["textDocumentSync"]["openClose"],
        true
    );
    assert!(
        initialized["capabilities"]
            .get("semanticTokensProvider")
            .is_none()
    );
    assert!(
        initialized["capabilities"]
            .get("inlineCompletionProvider")
            .is_none()
    );
    assert!(initialized["capabilities"].get("experimental").is_none());
    assert!(
        initialized["capabilities"]["workspace"]
            .get("fileOperations")
            .is_none()
    );
    session.notify("initialized", json!({}));
    open(&mut session, &fixture, "document.luau");
    for method in [
        "textDocument/semanticTokens/full",
        "textDocument/inlineCompletion",
        "textDocument/rangesFormatting",
    ] {
        let rejected = session.response(method, json!({"textDocument":{"uri":fixture.uri("document.luau")},"position":{"line":0,"character":0},"context":{"triggerKind":1},"ranges":[],"options":{"tabSize":2,"insertSpaces":true}}));
        assert_eq!(rejected["error"]["code"], -32602, "{method}: {rejected}");
    }
    let repeated = session.response("initialize", json!({"capabilities":{}}));
    assert_eq!(repeated["error"]["code"], -32600);
    snapshot(
        &fixture,
        "server",
        "negotiation",
        &json!({"capabilities": initialized["capabilities"]}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn synchronization() {
    let projections = Fixture::empty();
    let mut encodings = Vec::new();
    for (encoding, width) in [("utf-8", 4), ("utf-16", 2), ("utf-32", 1)] {
        let fixture = Fixture::new("cli", "project");
        let original = "local banner = \"😀\"; local count: number = 1\r\nreturn count, banner\r\n";
        fixture.write("protocol.luau", original);
        let mut supported = capabilities();
        supported["general"]["positionEncodings"] = json!([encoding]);
        let mut session = connect(&fixture, supported);
        open(&mut session, &fixture, "protocol.luau");
        let address = fixture.uri("protocol.luau");
        let changed = session.request(
            "textDocument/hover",
            json!({"textDocument":{"uri":address},"position":{"line":1,"character":7}}),
        );
        assert!(
            changed["contents"]["value"]
                .as_str()
                .expect("hover text")
                .contains("number")
        );
        assert_eq!(
            changed["range"],
            json!({"start":{"line":1,"character":7},"end":{"line":1,"character":12}})
        );
        let start = original.find('😀').expect("unicode source");
        session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"range":{"start":{"line":0,"character":start},"end":{"line":0,"character":start+width}},"text":"𝑥"},{"range":{"start":{"line":1,"character":7},"end":{"line":1,"character":12}},"text":"count"}]}));
        let synchronized = original.replace('😀', "𝑥");
        assert_eq!(
            session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
            synchronized
        );
        session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return false"}]}));
        let rejected = session.response("workspace/textDocumentContent", json!({"uri":address}));
        assert_eq!(rejected["error"]["code"], -32602);
        assert!(
            rejected["error"]["message"]
                .as_str()
                .expect("synchronization error")
                .contains("resynchronization")
        );
        session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":3},"contentChanges":[{"text":synchronized}]}));
        assert_eq!(
            session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
            synchronized
        );
        if encoding != "utf-32" {
            session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":4},"contentChanges":[{"range":{"start":{"line":0,"character":start+1},"end":{"line":0,"character":start+width}},"text":"x"}]}));
            let invalid = session.response(
                "textDocument/hover",
                json!({"textDocument":{"uri":address},"position":{"line":1,"character":7}}),
            );
            assert_eq!(invalid["error"]["code"], -32602);
            session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":5},"contentChanges":[{"text":synchronized}]}));
            assert_eq!(
                session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
                synchronized
            );
        }
        session.notify(
            "textDocument/didClose",
            json!({"textDocument":{"uri":address}}),
        );
        assert_eq!(
            session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
            original
        );
        assert_eq!(fixture.read("protocol.luau"), original);
        encodings.push(fixture.normalize(&json!({
            "encoding":encoding,
            "hover":changed,
            "resynchronization":rejected,
        })));
        assert_eq!(session.finish(), 0);
    }
    snapshot(&projections, "server", "synchronization", &json!(encodings));
}

#[test]
fn saving() {
    let fixture = Fixture::new("cli", "server");
    fixture.write("instar.toml", "[environment]\nroblox = false\n[editing]\nformat_on_save = true\n[formatting]\nindent_style = \"spaces\"\nindent_width = 2\n");
    let mut session = connect(&fixture, capabilities());
    let original = open(&mut session, &fixture, "document.luau");
    let address = fixture.uri("document.luau");
    session.notify(
        "textDocument/willSave",
        json!({"textDocument":{"uri":address},"reason":1}),
    );
    let edits = session.request(
        "textDocument/willSaveWaitUntil",
        json!({"textDocument":{"uri":address},"reason":1}),
    );
    let formatted = apply(&original, &edits, "utf-16");
    assert_eq!(formatted, "local value = { 1, 2, 3 }\nreturn value\n");
    assert_eq!(fixture.read("document.luau"), original);
    session.notify(
        "textDocument/didChange",
        json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":formatted}]}),
    );
    assert_eq!(
        session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
        formatted
    );
    fixture.write("document.luau", &formatted);
    session.notify(
        "textDocument/didSave",
        json!({"textDocument":{"uri":address},"text":formatted}),
    );
    assert_eq!(
        session.request(
            "textDocument/willSaveWaitUntil",
            json!({"textDocument":{"uri":address},"reason":1})
        ),
        Value::Null
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn negotiated() {
    let fixture = Fixture::new("cli", "project");
    let mut session = Session::new(&fixture);
    let mut supported = capabilities();
    supported["general"]["positionEncodings"] = json!(["utf-32", "utf-8"]);
    supported["textDocument"]["semanticTokens"]["tokenTypes"] =
        json!(["variable", "keyword", "unsupported"]);
    supported["textDocument"]["semanticTokens"]["tokenModifiers"] =
        json!(["declaration", "unsupported"]);
    let initialized = session.request("initialize", json!({"processId":null,"workspaceFolders":[{"uri":fixture.uri(""),"name":"contract"}],"capabilities":supported}));
    let advertised = &initialized["capabilities"];
    assert_eq!(advertised["positionEncoding"], "utf-32");
    assert_eq!(
        advertised["semanticTokensProvider"]["legend"],
        json!({"tokenTypes":["keyword","variable"],"tokenModifiers":["declaration"]})
    );
    assert_eq!(advertised["semanticTokensProvider"]["full"]["delta"], true);
    assert_eq!(advertised["semanticTokensProvider"]["range"], true);
    assert_eq!(advertised["experimental"]["rangesFormatting"], true);
    assert!(advertised.get("inlineCompletionProvider").is_some());
    assert_eq!(advertised["inlayHintProvider"]["resolveProvider"], true);
    assert_eq!(advertised["renameProvider"]["prepareProvider"], true);
    assert_eq!(
        advertised["workspaceSymbolProvider"]["resolveProvider"],
        true
    );
    assert_eq!(
        advertised["codeActionProvider"]["codeActionKinds"],
        json!([
            "quickfix",
            "source.fixAll.instar",
            "source.organizeImports",
            "refactor.extract"
        ])
    );
    assert_eq!(
        advertised["documentOnTypeFormattingProvider"]["firstTriggerCharacter"],
        "\n"
    );
    assert_eq!(
        advertised["documentOnTypeFormattingProvider"]["moreTriggerCharacter"],
        json!([";"])
    );
    assert_eq!(
        advertised["workspace"]["textDocumentContent"]["schemes"],
        json!(["instar", "untitled"])
    );
    let file_operations = advertised["workspace"]["fileOperations"]
        .as_object()
        .expect("negotiated file operations");
    assert_eq!(file_operations.len(), 6);
    for operation in [
        "didCreate",
        "willCreate",
        "didRename",
        "willRename",
        "didDelete",
        "willDelete",
    ] {
        assert_eq!(
            file_operations[operation],
            json!({"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]})
        );
    }
    assert!(
        advertised["executeCommandProvider"]["commands"]
            .as_array()
            .expect("advertised commands")
            .iter()
            .any(|command| command == "instar.applyFix")
    );
    session.notify("initialized", json!({}));
    let negotiated = initialized;
    assert_eq!(session.finish(), 0);

    let mut session = Session::new(&fixture);
    let initialized = session.request(
        "initialize",
        json!({"processId":null,"workspaceFolders":[{"uri":fixture.uri(""),"name":"contract"}],"capabilities":{"workspace":{"fileOperations":{"didCreate":true,"willCreate":false,"didRename":true,"willRename":false,"didDelete":true}}}}),
    );
    assert_eq!(
        initialized["capabilities"]["workspace"]["fileOperations"],
        json!({
            "didCreate":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]},
            "didRename":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]},
            "didDelete":{"filters":[{"scheme":"file","pattern":{"glob":"**/*"}}]}
        })
    );
    session.notify("initialized", json!({}));
    assert_eq!(session.finish(), 0);
    snapshot(
        &fixture,
        "server",
        "negotiated",
        &json!({"capabilities": negotiated["capabilities"]}),
    );
}
