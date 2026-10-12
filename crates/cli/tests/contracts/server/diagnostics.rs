use serde_json::{Value, json};

use super::{
    super::support::{Fixture, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn fidelity() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    let source = "local marker = \"😀\"; local value: string = 12\nreturn marker, value\n";
    fixture.write("protocol.luau", source);
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");

    let diagnostics = session.request(
        "textDocument/diagnostic",
        &json!({"textDocument":{"uri":address}}),
    );

    assert_eq!(diagnostics["kind"], "full");

    let diagnostic = diagnostics["items"]
        .as_array()
        .expect("document diagnostics")
        .iter()
        .find(|item| {
            item["source"] == "Luau"
                && item["severity"] == 1
                && item["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("number") && message.contains("string"))
        })
        .expect("real Luau number-to-string error");

    let start = source[..source.find("12").expect("initializer")]
        .encode_utf16()
        .count();

    assert_eq!(
        diagnostic["range"],
        json!({"start":{"line":0,"character":start},"end":{"line":0,"character":start+2}})
    );

    assert_eq!(diagnostic["code"], 1000);

    assert_eq!(
        diagnostic["message"],
        "Expected this to be 'string', but got 'number'"
    );

    let unchanged = session.request(
        "textDocument/diagnostic",
        &json!({"textDocument":{"uri":address},"previousResultId":diagnostics["resultId"]}),
    );

    assert_eq!(unchanged["kind"], "unchanged");
    assert_eq!(unchanged["resultId"], diagnostics["resultId"]);
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":source.replace("= 12", "= \"twelve\"")}]}));

    let corrected = session.request(
        "textDocument/diagnostic",
        &json!({"textDocument":{"uri":address},"previousResultId":diagnostics["resultId"]}),
    );

    assert_eq!(corrected["kind"], "full");

    assert!(
        !corrected["items"]
            .as_array()
            .expect("corrected findings")
            .iter()
            .any(|item| item["severity"] == 1)
    );

    assert_ne!(corrected["resultId"], diagnostics["resultId"]);

    let unknown = session.response(
        "textDocument/diagnostic",
        &json!({"textDocument":{"uri":address},"identifier":"unknown"}),
    );

    assert_eq!(unknown["error"]["code"], -32602);

    snapshot(
        &fixture,
        "server",
        "diagnostics",
        &json!({"initial":diagnostics,"unchanged":unchanged,"corrected":corrected}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn publication() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    fixture.write("protocol.luau", "local value: string = 12\nreturn value\n");
    let mut supported = capabilities();

    supported["textDocument"]
        .as_object_mut()
        .expect("document capabilities")
        .remove("diagnostic");

    let mut session = connect(&fixture, &supported);
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");

    let initial = loop {
        let publication = session.message("textDocument/publishDiagnostics")["params"].clone();

        if publication["uri"] == address && publication["version"] == 1 {
            break publication;
        }
    };

    assert!(
        initial["diagnostics"]
            .as_array()
            .expect("pushed diagnostics")
            .iter()
            .any(|diagnostic| diagnostic["severity"] == 1 && diagnostic["source"] == "Luau")
    );

    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"local value: string = \"valid\"\nreturn value\n"}]}));

    let updated = loop {
        let publication = session.message("textDocument/publishDiagnostics")["params"].clone();

        if publication["uri"] == address && publication["version"] == 2 {
            break publication;
        }
    };

    assert_eq!(updated["diagnostics"], json!([]));

    fixture.write(
        "protocol.luau",
        "local value: string = \"valid\"\nreturn value\n",
    );

    session.notify(
        "textDocument/didClose",
        &json!({"textDocument":{"uri":address}}),
    );

    let closed = loop {
        let publication = session.message("textDocument/publishDiagnostics")["params"].clone();

        if publication["uri"] == address && publication["version"].is_null() {
            break publication;
        }
    };

    assert_eq!(closed["diagnostics"], json!([]));

    snapshot(
        &fixture,
        "server",
        "publication",
        &json!({"initial":initial,"updated":updated,"closed":closed}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn workspace() {
    let fixture = Fixture::new("cli", "project");
    fixture.write(".luaurc", "{\"languageMode\":\"strict\"}\n");
    fixture.write("protocol.luau", "local value: string = 12\nreturn value\n");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let complete = session.request("workspace/diagnostic", &json!({"previousResultIds":[]}));

    let document = complete["items"]
        .as_array()
        .expect("workspace diagnostics")
        .iter()
        .find(|item| item["uri"] == address)
        .expect("opened workspace source");

    assert_eq!(document["version"], 1);
    assert_eq!(document["kind"], "full");

    assert!(
        document["items"]
            .as_array()
            .expect("source findings")
            .iter()
            .any(|item| item["severity"] == 1)
    );

    let partial = session.request(
        "workspace/diagnostic",
        &json!({"previousResultIds":[],"partialResultToken":"diagnostics"}),
    );

    assert_eq!(partial["items"], json!([]));
    let progress = session.message("$/progress")["params"].clone();
    assert_eq!(progress["token"], "diagnostics");

    let delivered = progress["value"]["items"]
        .as_array()
        .expect("partial diagnostic reports")
        .iter()
        .find(|item| item["uri"] == address)
        .expect("partial opened source");

    assert_eq!(delivered["items"], document["items"]);
    assert_eq!(delivered["version"], 1);

    let unknown = session.response(
        "workspace/diagnostic",
        &json!({"identifier":"unknown","previousResultIds":[]}),
    );

    assert_eq!(unknown["error"]["code"], -32602);

    snapshot(
        &fixture,
        "server",
        "workspace",
        &json!({"complete":complete,"partial":progress}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn prerequisites() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("instar.toml", "[environment]\nroblox = false\n[[environment.groups]]\nnamespace = \"@missing\"\ndefinitions = [\"missing.luau\"]\n");
    fixture.write("protocol.luau", "local value = 1\nreturn value\n");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let unavailable = session.response("textDocument/hover", &json!({"textDocument":{"uri":fixture.uri("protocol.luau")},"position":{"line":1,"character":7}}));

    assert!(
        unavailable["error"]["code"]
            .as_i64()
            .is_some_and(|code| matches!(code, -32602 | -32803))
    );

    assert!(
        unavailable["error"]["message"]
            .as_str()
            .expect("unavailable prerequisite message")
            .contains("missing.luau")
    );

    assert_ne!(unavailable["error"]["data"], Value::Null);
    assert!(unavailable.get("result").is_none());

    let outcomes = unavailable["error"]["data"]["completed_sources"]
        .as_array()
        .expect("retained source coverage");

    assert_eq!(outcomes.len(), 1);
    assert!(outcomes[0]["source"]["identity"].is_u64());
    assert_eq!(outcomes[0]["source"]["revision"], 1);

    assert_eq!(
        outcomes[0]["completion"],
        json!({"Incomplete":["UnavailableEnvironment"]})
    );

    snapshot(&fixture, "server", "prerequisites", &unavailable);
    assert_eq!(session.finish(), 0);
}
