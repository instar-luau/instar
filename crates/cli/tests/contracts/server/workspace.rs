use std::{fmt::Write, fs};

use serde_json::{Value, json};

use super::{
    super::support::{Fixture, apply, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn invalidation() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    fixture.write("dependency.luau", "return {value = 1}\n");
    fixture.write("protocol.luau", "local dependency = require(\"./dependency\")\nlocal value: number = dependency.value\nreturn value\n");
    let mut session = connect(&fixture, capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let position = json!({"textDocument":{"uri":address},"position":{"line":1,"character":35}});
    let numeric = session.request("textDocument/hover", position.clone());
    assert!(
        numeric["contents"]["value"]
            .as_str()
            .expect("original imported type")
            .contains("number")
    );
    let links = session.request(
        "textDocument/documentLink",
        json!({"textDocument":{"uri":address}}),
    );
    let link = links
        .as_array()
        .expect("dependency link")
        .iter()
        .find(|link| link["target"] == fixture.uri("dependency.luau"))
        .expect("resolved dependency");
    fixture.write("dependency.luau", "return {value = \"changed\"}\n");
    session.notify(
        "workspace/didChangeWatchedFiles",
        json!({"changes":[{"uri":fixture.uri("dependency.luau"),"type":2}]}),
    );
    let changed = session.request("textDocument/hover", position);
    assert!(
        changed["contents"]["value"]
            .as_str()
            .expect("invalidated imported type")
            .contains("string")
    );
    assert_eq!(
        session.response("documentLink/resolve", link.clone())["error"]["code"],
        -32801
    );
    let initial = session.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":address}}),
    );
    assert!(
        initial["items"]
            .as_array()
            .expect("dependency mismatch")
            .iter()
            .any(|finding| finding["severity"] == 1
                && finding["message"].as_str().is_some_and(
                    |message| message.contains("string") && message.contains("number")
                ))
    );
    session.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":{"instar":{"checking":{"type_errors":false}}}}),
    );
    let disabled = session.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":address},"previousResultId":initial["resultId"]}),
    );
    assert_eq!(disabled["kind"], "full");
    assert!(
        !disabled["items"]
            .as_array()
            .expect("disabled type diagnostics")
            .iter()
            .any(|finding| finding["severity"] == 1 && finding["source"] == "Luau")
    );
    session.notify(
        "workspace/didChangeConfiguration",
        json!({"settings":{"instar":{"checking":{"type_errors":true}}}}),
    );
    let restored = session.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":address},"previousResultId":disabled["resultId"]}),
    );
    assert_eq!(restored["kind"], "full");
    assert_eq!(restored["items"], initial["items"]);
    snapshot(
        &fixture,
        "server",
        "invalidation",
        &json!({"numeric":numeric,"changed":changed,"initial":initial,"disabled":disabled,"restored":restored}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn files() {
    let fixture = Fixture::new("cli", "movement");
    for name in ["capture.luau", "expected.luau", "moved.luau"] {
        fs::remove_file(fixture.path(name)).expect("remove separate rejection and golden inputs");
    }
    let mut session = connect(&fixture, capabilities());
    let main = open(&mut session, &fixture, "main.luau");
    let consumer = open(&mut session, &fixture, "consumer.luau");
    open(&mut session, &fixture, "dependency.luau");
    let creation = session.request(
        "workspace/willCreateFiles",
        json!({"files":[{"uri":fixture.uri("created.lua") }]}),
    );
    assert_eq!(creation["documentChanges"], json!([]));
    assert!(!fixture.path("created.lua").exists());
    fixture.write("created.lua", "local created = 1\nreturn created\n");
    session.notify(
        "workspace/didCreateFiles",
        json!({"files":[{"uri":fixture.uri("created.lua")}]}),
    );
    let created = session.request("workspace/symbol", json!({"query":"created"}));
    assert!(
        created
            .as_array()
            .expect("created symbol index")
            .iter()
            .any(
                |symbol| symbol["location"]["uri"] == fixture.uri("created.lua")
                    && symbol["name"] == "created"
            )
    );
    let preflight = session.request("workspace/willRenameFiles", json!({"files":[{"oldUri":fixture.uri("dependency.luau"),"newUri":fixture.uri("provider.luau")}]}));
    let changes = preflight["documentChanges"]
        .as_array()
        .expect("guarded import movement edits");
    assert_eq!(changes.len(), 2);
    for change in changes {
        assert_eq!(change["textDocument"]["version"], 1);
        let (name, source) = if change["textDocument"]["uri"] == fixture.uri("main.luau") {
            ("main.luau", &main)
        } else {
            assert_eq!(change["textDocument"]["uri"], fixture.uri("consumer.luau"));
            ("consumer.luau", &consumer)
        };
        let updated = apply(source, &change["edits"], "utf-16");
        assert_eq!(updated, source.replace("/dependency", "/provider"));
        assert_eq!(fixture.read(name), *source);
        fixture.write(name, &updated);
        session.notify("textDocument/didChange", json!({"textDocument":{"uri":fixture.uri(name),"version":2},"contentChanges":[{"text":updated}]}));
    }
    fs::rename(
        fixture.path("dependency.luau"),
        fixture.path("provider.luau"),
    )
    .expect("perform fixture file movement");
    session.notify("workspace/didRenameFiles", json!({"files":[{"oldUri":fixture.uri("dependency.luau"),"newUri":fixture.uri("provider.luau")}]}));
    assert_eq!(
        session.request(
            "workspace/textDocumentContent",
            json!({"uri":fixture.uri("provider.luau")})
        )["text"],
        fixture.read("provider.luau")
    );
    let diagnostics = session.request(
        "textDocument/diagnostic",
        json!({"textDocument":{"uri":fixture.uri("main.luau")}}),
    );
    assert!(
        !diagnostics["items"]
            .as_array()
            .expect("moved import diagnostics")
            .iter()
            .any(|finding| finding["severity"] == 1)
    );
    let deletion = session.request(
        "workspace/willDeleteFiles",
        json!({"files":[{"uri":fixture.uri("created.lua")}]}),
    );
    assert_eq!(deletion["documentChanges"], json!([]));
    assert!(fixture.path("created.lua").exists());
    fs::remove_file(fixture.path("created.lua")).expect("perform fixture file deletion");
    session.notify(
        "workspace/didDeleteFiles",
        json!({"files":[{"uri":fixture.uri("created.lua")}]}),
    );
    assert!(
        !session
            .request("workspace/symbol", json!({"query":"created"}))
            .as_array()
            .expect("deleted symbol index")
            .iter()
            .any(|symbol| symbol["location"]["uri"] == fixture.uri("created.lua"))
    );
    let protected = session.response(
        "workspace/willDeleteFiles",
        json!({"files":[{"uri":fixture.uri("provider.luau")}]}),
    );
    assert!(protected.get("error").is_some());
    assert!(
        protected["error"]["message"]
            .as_str()
            .expect("referenced module protection")
            .contains("referenced module")
    );
    assert!(fixture.path("provider.luau").exists());
    assert_eq!(
        session.response(
            "workspace/willCreateFiles",
            json!({"files":[{"uri":fixture.uri("provider.luau")}]})
        )["error"]["code"],
        -32602
    );
    snapshot(
        &fixture,
        "server",
        "files",
        &json!({"creation":creation,"rename":preflight,"deletion":deletion,"protected":protected}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn folders() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(
        "external/entry.luau",
        "local externalmarker = 1\nreturn externalmarker\n",
    );
    let mut session = connect(&fixture, capabilities());
    let initial = session.request("workspace/symbol", json!({"query":"double"}));
    assert!(
        initial
            .as_array()
            .expect("original workspace index")
            .iter()
            .any(|symbol| symbol["name"] == "double"
                && symbol["location"]["uri"] == fixture.uri("library.luau"))
    );
    session.notify("workspace/didChangeWorkspaceFolders", json!({"event":{"removed":[{"uri":fixture.uri(""),"name":"contract"}],"added":[{"uri":fixture.uri("external"),"name":"external"}]}}));
    assert_eq!(
        session.request("workspace/symbol", json!({"query":"double"})),
        json!([])
    );
    let external = session.request("workspace/symbol", json!({"query":"externalmarker"}));
    assert!(
        external
            .as_array()
            .expect("added workspace root")
            .iter()
            .any(|symbol| symbol["name"] == "externalmarker"
                && symbol["location"]["uri"] == fixture.uri("external/entry.luau"))
    );
    session.notify(
        "workspace/didChangeWorkspaceFolders",
        json!({"event":{"removed":[{"uri":fixture.uri("external"),"name":"external"}],"added":[]}}),
    );
    assert_eq!(
        session.request("workspace/symbol", json!({"query":"externalmarker"})),
        json!([])
    );
    snapshot(
        &fixture,
        "server",
        "folders",
        &json!({"initial":initial,"external":external}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn integration() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    fixture.write(
        "protocol.luau",
        "local function problem(): string\n    return 1\nend\nreturn problem()\n",
    );
    let mut supported = capabilities();
    supported["workspace"]["configuration"] = json!(true);
    supported["workspace"]["workspaceFolders"] = json!(true);
    supported["workspace"]["didChangeWatchedFiles"] = json!({"dynamicRegistration":true});
    for capability in [
        "diagnostics",
        "semanticTokens",
        "inlayHint",
        "codeLens",
        "inlineValue",
        "foldingRange",
    ] {
        supported["workspace"][capability] = json!({"refreshSupport":true});
    }
    supported["window"] = json!({"showDocument":{"support":true}});
    let mut session = connect(&fixture, supported);
    let registration = session.message("client/registerCapability");
    assert_eq!(
        registration["params"]["registrations"][0]["method"],
        "workspace/didChangeWatchedFiles"
    );
    assert_eq!(
        registration["params"]["registrations"][0]["registerOptions"]["watchers"],
        json!([{"globPattern":"**/*"}])
    );
    session.message("workspace/workspaceFolders");
    let configuration = session.message("workspace/configuration");
    assert_eq!(
        configuration["params"]["items"],
        json!([{"scopeUri":fixture.uri(""),"section":"instar"}])
    );
    open(&mut session, &fixture, "protocol.luau");
    for method in [
        "workspace/diagnostic/refresh",
        "workspace/semanticTokens/refresh",
        "workspace/inlayHint/refresh",
        "workspace/codeLens/refresh",
        "workspace/inlineValue/refresh",
        "workspace/foldingRange/refresh",
    ] {
        let refresh = session.message(method);
        assert!(
            refresh.get("id").is_some(),
            "refresh must await a client response: {refresh}"
        );
        assert!(
            refresh["params"].is_null(),
            "parameterless refresh: {refresh}"
        );
    }
    session.notify("textDocument/didOpen", json!({"textDocument":{"uri":"untitled:protocol","languageId":"luau","version":1,"text":"local value = 1\nreturn value\n"}}));
    assert_eq!(
        session.request(
            "workspace/textDocumentContent",
            json!({"uri":"untitled:protocol"})
        )["text"],
        "local value = 1\nreturn value\n"
    );
    let content = session.message("workspace/textDocumentContent/refresh");
    assert_eq!(content["params"]["uri"], "untitled:protocol");
    session.notify(
        "textDocument/didSave",
        json!({"textDocument":{"uri":"untitled:protocol"}}),
    );
    let log = session.message("window/logMessage");
    let shown = session.message("window/showMessage");
    assert_eq!(log["params"]["type"], 1);
    assert_eq!(shown["params"]["type"], 1);
    assert!(
        log["params"]["message"]
            .as_str()
            .expect("save protection log")
            .contains("cannot be saved")
    );
    assert_eq!(shown["params"]["message"], log["params"]["message"]);
    session.reply("window/showMessageRequest", json!({"title":"Show source"}));
    let lenses = session.request(
        "textDocument/codeLens",
        json!({"textDocument":{"uri":fixture.uri("protocol.luau")}}),
    );
    let resolved = session.request("codeLens/resolve", lenses[0].clone());
    let checked = session.request("workspace/executeCommand", resolved["command"].clone());
    assert_eq!(checked["failed"], true);
    assert!(
        checked["findings"]
            .as_u64()
            .expect("failed checking findings")
            >= 1
    );
    assert_eq!(checked["completion"], "Complete");
    let choice = session.message("window/showMessageRequest");
    assert_eq!(
        choice["params"]["actions"],
        json!([{"title":"Show source"}])
    );
    let revealed = session.message("window/showDocument");
    assert_eq!(revealed["params"]["uri"], fixture.uri("protocol.luau"));
    assert_eq!(revealed["params"]["takeFocus"], true);
    assert_eq!(revealed["params"]["external"], false);
    assert_eq!(session.request("shutdown", Value::Null), Value::Null);
    let unregistration = session.message("client/unregisterCapability");
    let entries = unregistration["params"]["unregisterations"]
        .as_array()
        .expect("protocol unregistration list");
    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0]["id"],
        registration["params"]["registrations"][0]["id"]
    );
    assert_eq!(entries[0]["method"], "workspace/didChangeWatchedFiles");
    assert_eq!(
        session.response("workspace/symbol", json!({"query":"problem"}))["error"]["code"],
        -32600
    );
    snapshot(
        &fixture,
        "server",
        "integration",
        &json!({"registration":registration["params"],"configuration":configuration["params"],"content":content["params"],"log":log["params"],"choice":choice["params"],"reveal":revealed["params"],"unregistration":unregistration["params"]}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn current() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    fixture.write(
        "protocol.luau",
        "local function problem(): string\n    return 1\nend\nreturn problem()\n",
    );
    let mut supported = capabilities();
    supported["window"] = json!({"showDocument":{"support":true}});
    let mut session = connect(&fixture, supported);
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let lenses = session.request(
        "textDocument/codeLens",
        json!({"textDocument":{"uri":address}}),
    );
    let resolved = session.request("codeLens/resolve", lenses[0].clone());
    session.hold("window/showMessageRequest");
    let identifier = session.start("workspace/executeCommand", json!({"command":resolved["command"]["command"],"arguments":resolved["command"]["arguments"]}));
    let choice = session.message("window/showMessageRequest");
    assert_eq!(
        choice["params"]["actions"],
        json!([{"title":"Show source"}])
    );
    session.notify("textDocument/didChange", json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"local total = true\nreturn total\n"}]}));
    assert_eq!(
        session.request("workspace/textDocumentContent", json!({"uri":address}))["text"],
        "local total = true\nreturn total\n"
    );
    session.respond(choice["id"].clone(), json!({"title":"Show source"}));
    let stale = session.receive(identifier);
    assert_eq!(stale["error"]["code"], -32801);
    assert!(stale.get("result").is_none());
    snapshot(
        &fixture,
        "server",
        "current",
        &json!({"stale":stale,"choice":choice["params"]}),
    );
    assert_eq!(session.finish(), 0);
}

fn workload() -> String {
    let mut source = String::from("local total = 0\n");
    for value in 0..8000 {
        writeln!(
            source,
            "do local value: number = {value}; total += value end"
        )
        .expect("write real Luau workload");
    }
    source.push_str("return total\n");
    source
}

#[test]
fn progress() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("protocol.luau", workload());
    let mut supported = capabilities();
    supported["window"] = json!({"workDoneProgress":true});
    let mut session = connect(&fixture, supported);
    open(&mut session, &fixture, "protocol.luau");
    let identifier = session.start("textDocument/hover", json!({"textDocument":{"uri":fixture.uri("protocol.luau")},"position":{"line":8001,"character":7},"workDoneToken":"analysis"}));
    let begin = loop {
        let progress = session.message("$/progress");
        if progress["params"]["token"] == "analysis" {
            break progress;
        }
    };
    assert_eq!(begin["params"]["value"]["kind"], "begin");
    assert_eq!(begin["params"]["value"]["cancellable"], true);
    let response = session.receive(identifier);
    assert!(response.get("error").is_none(), "{response}");
    assert!(
        response["result"]["contents"]["value"]
            .as_str()
            .expect("completed workload hover")
            .contains("number")
    );
    snapshot(
        &fixture,
        "server",
        "progress",
        &json!({"begin":begin["params"],"result":response["result"]}),
    );
    assert_eq!(session.finish(), 0);
}
