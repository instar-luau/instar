use serde_json::json;

use super::{
    super::support::{Fixture, apply, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn documentation() {
    let fixture = Fixture::new("cli", "hover");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "documented.luau");
    let hover = session.request("textDocument/hover", &json!({"textDocument":{"uri":fixture.uri("documented.luau")},"position":{"line":0,"character":9}}));
    assert_eq!(hover["contents"]["kind"], "markdown");

    let contents = hover["contents"]["value"]
        .as_str()
        .expect("documented hover");

    for required in [
        "number",
        "string",
        "Deprecated.",
        "Explains the supplied **number**.",
        "explain(42)",
        "https://example.invalid/explain",
    ] {
        assert!(
            contents.contains(required),
            "missing {required}: {contents}"
        );
    }

    assert_eq!(
        hover["range"],
        json!({"start":{"line":0,"character":7},"end":{"line":0,"character":14}})
    );

    snapshot(&fixture, "server", "documentation", &hover);
    assert_eq!(session.finish(), 0);
}

#[test]
fn completion() {
    let fixture = Fixture::new("cli", "completion");
    let mut session = connect(&fixture, &capabilities());
    let source = open(&mut session, &fixture, "member.luau");
    let address = fixture.uri("member.luau");
    let position = source.trim_end_matches(['\r', '\n']).encode_utf16().count();
    let completion = session.request("textDocument/completion", &json!({"textDocument":{"uri":address},"position":{"line":0,"character":position},"context":{"triggerKind":1}}));
    assert!(completion["isIncomplete"].is_boolean());
    let candidates = completion["items"].as_array().expect("member candidates");

    let value = candidates
        .iter()
        .find(|item| item["label"] == "value")
        .expect("known number property");

    assert!(
        value["detail"]
            .as_str()
            .expect("member type")
            .contains("number")
    );

    let odd = candidates
        .iter()
        .find(|item| item["label"] == "odd key")
        .expect("quoted property");

    let edited = apply(&source, &json!([odd["textEdit"]]), "utf-16");
    assert!(edited.trim_end().ends_with("record[\"odd key\"]"));
    let resolved = session.request("completionItem/resolve", odd);
    assert_eq!(resolved["data"], odd["data"]);
    assert_eq!(resolved["label"], odd["label"]);
    assert_eq!(resolved["textEdit"], odd["textEdit"]);
    let inline = session.request("textDocument/inlineCompletion", &json!({"textDocument":{"uri":address},"position":{"line":0,"character":position},"context":{"triggerKind":1}}));

    assert!(
        inline["items"]
            .as_array()
            .expect("inline proposals")
            .iter()
            .any(|item| item["insertText"] == "[\"odd key\"]")
    );

    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"local record = {other = true}\nreturn record\n"}]}));
    let stale = session.response("completionItem/resolve", odd);
    assert_eq!(stale["error"]["code"], -32801);

    snapshot(
        &fixture,
        "server",
        "completion",
        &json!({"member":value,"quoted":odd,"resolved":resolved,"inline":inline,"stale":stale}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn signatures() {
    let fixture = Fixture::new("cli", "signatures");
    let mut session = connect(&fixture, &capabilities());
    let source = open(&mut session, &fixture, "input.luau");
    let line = source.lines().nth(11).expect("variadic call");
    let argument = line.find(", 2").expect("second argument") + 2;
    let character = line[..argument].encode_utf16().count();
    let signatures = session.request("textDocument/signatureHelp", &json!({"textDocument":{"uri":fixture.uri("input.luau")},"position":{"line":11,"character":character},"context":{"triggerKind":1,"isRetrigger":false}}));
    assert_eq!(signatures["activeParameter"], 1);
    let signature = &signatures["signatures"][0];
    assert_eq!(signature["activeParameter"], 1);
    let label = signature["label"].as_str().expect("callable label");
    assert!(label.contains("string") && label.contains("number") && label.contains("boolean"));

    let offsets = signature["parameters"][1]["label"]
        .as_array()
        .expect("negotiated parameter offsets");

    let units = label.encode_utf16().collect::<Vec<_>>();

    let start = usize::try_from(offsets[0].as_u64().expect("parameter start"))
        .expect("bounded parameter start");

    let end = usize::try_from(offsets[1].as_u64().expect("parameter end"))
        .expect("bounded parameter end");

    assert!(
        String::from_utf16(&units[start..end])
            .expect("parameter label")
            .contains("number")
    );

    snapshot(&fixture, "server", "signatures", &signatures);
    assert_eq!(session.finish(), 0);
}

#[test]
fn annotations() {
    let fixture = Fixture::new("cli", "hints");
    let mut session = connect(&fixture, &capabilities());
    let source = open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");
    let hints = session.request("textDocument/inlayHint", &json!({"textDocument":{"uri":address},"range":{"start":{"line":0,"character":0},"end":{"line":7,"character":0}}}));

    let count = source
        .lines()
        .next()
        .expect("unicode first line")
        .find("count")
        .expect("inferred binding");

    let character = source[..count].encode_utf16().count() + "count".len();

    let hint = hints
        .as_array()
        .expect("inlay hints")
        .iter()
        .find(|hint| hint["position"] == json!({"line":0,"character":character}))
        .expect("inferred count hint");

    assert_eq!(hint["kind"], 1);
    assert_eq!(hint["label"], ": number");
    assert!(hint.get("tooltip").is_none());

    assert!(
        !hints
            .as_array()
            .expect("hints")
            .iter()
            .any(|hint| hint["position"]["line"] == 1)
    );

    let resolved = session.request("inlayHint/resolve", hint);
    assert_eq!(resolved["tooltip"], "count: number");
    assert_eq!(resolved["data"], hint["data"]);
    assert_eq!(resolved["position"], hint["position"]);
    let values = session.request("textDocument/inlineValue", &json!({"textDocument":{"uri":address},"range":{"start":{"line":0,"character":0},"end":{"line":7,"character":0}},"context":{"frameId":1,"stoppedLocation":{"start":{"line":6,"character":0},"end":{"line":6,"character":30}}}}));

    assert!(
        values
            .as_array()
            .expect("debug variable descriptors")
            .iter()
            .any(|value| value["variableName"] == "count" && value["caseSensitiveLookup"] == true)
    );

    assert!(
        values
            .as_array()
            .expect("debug descriptors")
            .iter()
            .all(|value| value.get("text").is_none() && value.get("value").is_none())
    );

    let crossed = session.response("codeLens/resolve", &json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"data":hint["data"]}));
    assert_eq!(crossed["error"]["code"], -32602);
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));

    assert_eq!(
        session.response("inlayHint/resolve", hint)["error"]["code"],
        -32801
    );

    snapshot(
        &fixture,
        "server",
        "annotations",
        &json!({"hint":hint,"resolved":resolved,"values":values,"crossed":crossed}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn lenses() {
    let fixture = Fixture::new("cli", "hierarchy");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");

    let lenses = session.request(
        "textDocument/codeLens",
        &json!({"textDocument":{"uri":address}}),
    );

    let lens = lenses
        .as_array()
        .expect("function lenses")
        .iter()
        .find(|lens| lens["range"]["start"]["line"] == 3)
        .expect("caller lens");

    assert_eq!(
        lens["range"],
        json!({"start":{"line":3,"character":15},"end":{"line":3,"character":21}})
    );

    assert!(lens.get("command").is_none());
    let resolved = session.request("codeLens/resolve", lens);
    assert_eq!(resolved["range"], lens["range"]);
    assert_eq!(resolved["data"], lens["data"]);
    assert_eq!(resolved["command"]["title"], "2 references");
    assert_eq!(resolved["command"]["command"], "instar.check");
    let checked = session.request("workspace/executeCommand", &json!({"command":resolved["command"]["command"],"arguments":resolved["command"]["arguments"]}));
    assert_eq!(checked["failed"], false);
    assert_eq!(checked["completion"], "Complete");
    let mut altered = resolved["command"].clone();
    altered["command"] = json!("instar.format");

    assert_eq!(
        session.response("workspace/executeCommand", &altered)["error"]["code"],
        -32602
    );

    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));

    assert_eq!(
        session.response("codeLens/resolve", lens)["error"]["code"],
        -32801
    );

    snapshot(
        &fixture,
        "server",
        "lenses",
        &json!({"lens":lens,"resolved":resolved,"checked":checked}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn imports() {
    let fixture = Fixture::new("cli", "completion");
    fixture.write("require.luau", "return require(\"./dep");
    let mut session = connect(&fixture, &capabilities());
    let source = open(&mut session, &fixture, "require.luau");
    let completion = session.request("textDocument/completion", &json!({"textDocument":{"uri":fixture.uri("require.luau")},"position":{"line":0,"character":source.len()},"context":{"triggerKind":1}}));
    assert_eq!(completion["isIncomplete"], true);

    let candidate = completion["items"]
        .as_array()
        .expect("filesystem require completions")
        .iter()
        .find(|candidate| candidate["label"] == "./dependency")
        .expect("existing dependency path");

    assert_eq!(candidate["textEdit"]["newText"], "\"./dependency\"");

    assert_eq!(
        candidate["textEdit"]["range"],
        json!({"start":{"line":0,"character":15},"end":{"line":0,"character":21}})
    );

    assert!(
        !completion["items"]
            .as_array()
            .expect("require paths")
            .iter()
            .any(|candidate| candidate["label"] == "./missing")
    );

    snapshot(&fixture, "server", "imports", &completion);
    assert_eq!(session.finish(), 0);
}
