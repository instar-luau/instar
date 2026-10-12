use serde_json::json;

use super::{
    super::support::{Fixture, apply, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn rename() {
    let fixture = Fixture::new("cli", "project");
    let source = "local original = 1\nlocal occupied = 2\ndo\n    local original = 3\n    print(original)\nend\nreturn original, occupied\n";
    fixture.write("protocol.luau", source);
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let position = json!({"textDocument":{"uri":address},"position":{"line":6,"character":9}});
    let prepared = session.request("textDocument/prepareRename", &position);

    assert_eq!(
        prepared,
        json!({"start":{"line":6,"character":7},"end":{"line":6,"character":15}})
    );

    let mut parameters = position.clone();
    parameters["newName"] = json!("renamed");
    let renamed = session.request("textDocument/rename", &parameters);

    let changes = renamed["documentChanges"]
        .as_array()
        .expect("versioned rename");

    assert_eq!(changes.len(), 1);

    assert_eq!(
        changes[0]["textDocument"],
        json!({"uri":address,"version":1})
    );

    assert_eq!(
        changes[0]["edits"].as_array().expect("rename edits").len(),
        2
    );

    assert_eq!(
        apply(source, &changes[0]["edits"], "utf-16"),
        "local renamed = 1\nlocal occupied = 2\ndo\n    local original = 3\n    print(original)\nend\nreturn renamed, occupied\n"
    );

    let linked = session.request("textDocument/linkedEditingRange", &position);

    assert_eq!(
        linked["ranges"],
        json!([
            {"start":{"line":0,"character":6},"end":{"line":0,"character":14}},
            {"start":{"line":6,"character":7},"end":{"line":6,"character":15}}
        ])
    );

    for name in ["occupied", "end", "invalid-name"] {
        parameters["newName"] = json!(name);
        let rejected = session.response("textDocument/rename", &parameters);

        let code = rejected["error"]["code"]
            .as_i64()
            .expect("rename rejection code");

        if name == "occupied" {
            assert!(
                matches!(code, -32602 | -32803),
                "rename to {name}: {rejected}"
            );
        } else {
            assert_eq!(code, -32602, "rename to {name}: {rejected}");
        }

        assert!(rejected.get("result").is_none());
    }

    assert_eq!(fixture.read("protocol.luau"), source);

    snapshot(
        &fixture,
        "server",
        "rename",
        &json!({"prepared":prepared,"rename":renamed,"linked":linked}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn fixes() {
    let fixture = Fixture::new("cli", "actions");
    let source = "local value = 1\nif(value > 0)then\n    value = value + 1\nend\nreturn value\n";
    fixture.write("input.luau", source);
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");
    let actions = session.request("textDocument/codeAction", &json!({"textDocument":{"uri":address},"range":{"start":{"line":0,"character":0},"end":{"line":5,"character":0}},"context":{"diagnostics":[],"only":["quickfix","source.fixAll.instar"]}}));
    let entries = actions.as_array().expect("rule actions");

    for kind in ["quickfix", "source.fixAll.instar"] {
        let action = entries
            .iter()
            .find(|action| action["kind"] == kind)
            .expect("selected action kind");

        assert!(action.get("edit").is_none());
        let resolved = session.request("codeAction/resolve", action);
        assert_eq!(resolved["data"], action["data"]);
        assert_eq!(resolved["kind"], kind);
        let edit = &resolved["edit"]["documentChanges"][0];
        assert_eq!(edit["textDocument"], json!({"uri":address,"version":1}));

        assert_eq!(
            apply(source, &edit["edits"], "utf-16"),
            "local value = 1\nif value > 0 then\n    value = value + 1\nend\nreturn value\n"
        );
    }

    let quick = entries
        .iter()
        .find(|action| action["kind"] == "quickfix")
        .expect("quick fix");

    assert!(
        quick["diagnostics"]
            .as_array()
            .expect("action diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["code"] == "parenthesized_conditions")
    );

    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));

    assert_eq!(
        session.response("codeAction/resolve", quick)["error"]["code"],
        -32801
    );

    assert_eq!(fixture.read("input.luau"), source);
    snapshot(&fixture, "server", "fixes", &actions);
    assert_eq!(session.finish(), 0);
}

#[test]
fn refactoring() {
    let fixture = Fixture::new("cli", "actions");
    fixture.write("first.luau", "return 1\n");
    fixture.write("second.luau", "return 2\n");
    fixture.write("imports.luau", "local second=require(\"./second\")\nlocal first=require(\"./first\")\nreturn first,second\n");
    let mut session = connect(&fixture, &capabilities());
    let imports = open(&mut session, &fixture, "imports.luau");
    let actions = session.request("textDocument/codeAction", &json!({"textDocument":{"uri":fixture.uri("imports.luau")},"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":0}},"context":{"diagnostics":[],"only":["source.organizeImports"]}}));

    assert_eq!(
        actions.as_array().expect("organize imports action").len(),
        1
    );

    assert_eq!(actions[0]["kind"], "source.organizeImports");
    let organized = session.request("codeAction/resolve", &actions[0]);
    let changes = &organized["edit"]["documentChanges"][0];
    assert_eq!(changes["textDocument"]["version"], 1);

    assert_eq!(
        apply(&imports, &changes["edits"], "utf-16"),
        "local first = require(\"./first\")\nlocal second = require(\"./second\")\nreturn first,second\n"
    );

    let literal = open(&mut session, &fixture, "literal.luau");
    let extraction = session.request("textDocument/codeAction", &json!({"textDocument":{"uri":fixture.uri("literal.luau")},"range":{"start":{"line":2,"character":14},"end":{"line":2,"character":16}},"context":{"diagnostics":[],"only":["refactor.extract"]}}));
    assert_eq!(extraction.as_array().expect("literal extraction").len(), 1);
    assert_eq!(extraction[0]["kind"], "refactor.extract");
    let extracted = session.request("codeAction/resolve", &extraction[0]);

    assert_eq!(
        apply(
            &literal,
            &extracted["edit"]["documentChanges"][0]["edits"],
            "utf-16"
        ),
        "local extracted = 1\nlocal label = \"😀\"\nlocal extracted1 = 42\nlocal value = extracted1\nreturn value + extracted, label\n"
    );

    assert_eq!(fixture.read("literal.luau"), literal);

    snapshot(
        &fixture,
        "server",
        "refactoring",
        &json!({"imports":organized,"literal":extracted}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn application() {
    let fixture = Fixture::new("cli", "actions");
    let source = "local value = 1\nif(value > 0)then\n    value = value + 1\nend\nreturn value\n";
    fixture.write("input.luau", source);
    let mut supported = capabilities();

    supported["textDocument"]
        .as_object_mut()
        .expect("document capabilities")
        .remove("codeAction");

    let mut session = connect(&fixture, &supported);
    open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");
    let commands = session.request("textDocument/codeAction", &json!({"textDocument":{"uri":address},"range":{"start":{"line":0,"character":0},"end":{"line":5,"character":0}},"context":{"diagnostics":[],"only":["quickfix"]}}));

    let command = commands
        .as_array()
        .expect("command fallback")
        .iter()
        .find(|command| command["command"] == "instar.applyFix")
        .expect("explicit apply-fix intent");

    let rejected = session.request("workspace/executeCommand", command);

    assert_eq!(
        rejected,
        json!({"applied":false,"failureReason":"contract client declined edit"})
    );

    let request = session.message("workspace/applyEdit");
    assert_eq!(request["params"]["label"], "instar.applyFix");

    assert_eq!(
        request["params"]["edit"]["documentChanges"][0]["textDocument"],
        json!({"uri":address,"version":1})
    );

    assert_eq!(fixture.read("input.luau"), source);
    session.hold("workspace/applyEdit");
    let identifier = session.start("workspace/executeCommand", command);
    let request = session.message("workspace/applyEdit");
    let edits = &request["params"]["edit"]["documentChanges"][0]["edits"];
    let applied = apply(source, edits, "utf-16");

    assert_eq!(
        applied,
        "local value = 1\nif value > 0 then\n    value = value + 1\nend\nreturn value\n"
    );

    fixture.write("input.luau", &applied);

    session.notify(
        "textDocument/didChange",
        &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":applied}]}),
    );

    session.respond(&request["id"], &json!({"applied":true}));

    assert_eq!(
        session.receive(identifier)["result"],
        json!({"applied":true})
    );

    assert_eq!(
        session.response("workspace/executeCommand", command)["error"]["code"],
        -32801
    );

    assert_eq!(
        session.request("workspace/textDocumentContent", &json!({"uri":address}))["text"],
        applied
    );

    snapshot(
        &fixture,
        "server",
        "application",
        &json!({"command":command,"declined":rejected,"accepted":request["params"]}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn closed() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("dependency.luau", "return {value = 1}\n");
    fixture.write("secondary.luau", "return 2\n");
    fixture.write("protocol.luau", "local secondary=require(\"./secondary\")\nlocal dependency=require(\"./dependency\")\nreturn dependency.value, secondary\n");
    let mut supported = capabilities();

    supported["textDocument"]
        .as_object_mut()
        .expect("document capabilities")
        .remove("codeAction");

    let mut session = connect(&fixture, &supported);
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let actions = session.request("textDocument/codeAction", &json!({"textDocument":{"uri":address},"range":{"start":{"line":0,"character":0},"end":{"line":3,"character":0}},"context":{"diagnostics":[],"only":["source.organizeImports"]}}));
    assert_eq!(actions.as_array().expect("guarded import command").len(), 1);
    assert_eq!(actions[0]["command"], "instar.organizeImports");

    session.notify(
        "textDocument/didClose",
        &json!({"textDocument":{"uri":address}}),
    );

    fixture.write("protocol.luau", "return \"externally changed\"\n");

    assert_eq!(
        session.response("workspace/executeCommand", &actions[0])["error"]["code"],
        -32801
    );

    let unknown = session.response(
        "workspace/executeCommand",
        &json!({"command":"instar.unknown","arguments":[]}),
    );

    assert_eq!(unknown["error"]["code"], -32602);

    let unguarded = session.response(
        "workspace/executeCommand",
        &json!({"command":"instar.format","arguments":[{"source":1}]}),
    );

    assert_eq!(unguarded["error"]["code"], -32602);

    assert_eq!(
        fixture.read("protocol.luau"),
        "return \"externally changed\"\n"
    );

    assert_eq!(session.finish(), 0);
}
