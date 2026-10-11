use serde_json::{Value, json};

use super::{
    super::support::{Fixture, apply, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn formatting() {
    let fixture = Fixture::new("cli", "server");
    let source = "local first={1,2}\nlocal second={3,4}\nreturn first,second\n";
    fixture.write("document.luau", source);
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "document.luau");
    let address = fixture.uri("document.luau");
    let options = json!({"tabSize":2,"insertSpaces":true});
    let parameters = json!({"textDocument":{"uri":address},"options":options});
    let whole = session.request("textDocument/formatting", &parameters);
    let formatted = "local first = { 1, 2 }\nlocal second = { 3, 4 }\nreturn first, second\n";
    assert_eq!(apply(source, &whole, "utf-16"), formatted);
    let mut selected = parameters.clone();
    selected["range"] = json!({"start":{"line":0,"character":12},"end":{"line":0,"character":15}});
    let ranged = session.request("textDocument/rangeFormatting", &selected);
    assert_eq!(
        apply(source, &ranged, "utf-16"),
        "local first = { 1, 2 }\nlocal second={3,4}\nreturn first,second\n"
    );
    let mut multiple = parameters.clone();
    multiple["ranges"] = json!([
        {"start":{"line":0,"character":0},"end":{"line":0,"character":16}},
        {"start":{"line":1,"character":0},"end":{"line":1,"character":17}}
    ]);
    let regions = session.request("textDocument/rangesFormatting", &multiple);
    assert_eq!(
        apply(source, &regions, "utf-16"),
        "local first = { 1, 2 }\nlocal second = { 3, 4 }\nreturn first,second\n"
    );
    let typed = session.request("textDocument/onTypeFormatting", &json!({"textDocument":{"uri":address},"position":{"line":1,"character":0},"ch":"\n","options":options}));
    assert_eq!(
        apply(source, &typed, "utf-16"),
        "local first = { 1, 2 }\nlocal second={3,4}\nreturn first,second\n"
    );
    let unsupported = session.response("textDocument/onTypeFormatting", &json!({"textDocument":{"uri":address},"position":{"line":1,"character":0},"ch":"x","options":options}));
    assert_eq!(unsupported["error"]["code"], -32602);
    let mut invalid = parameters.clone();
    invalid["options"]["tabSize"] = json!(0);
    assert_eq!(
        session.response("textDocument/formatting", &invalid)["error"]["code"],
        -32602
    );
    assert_eq!(fixture.read("document.luau"), source);
    session.notify(
        "textDocument/didChange",
        &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":formatted}]}),
    );
    assert_eq!(
        session.request("textDocument/formatting", &parameters),
        Value::Null
    );
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":3},"contentChanges":[{"text":"local first = {\nreturn first\n"}]}));
    let malformed = session.response("textDocument/formatting", &parameters);
    assert_eq!(malformed["error"]["code"], -32602);
    assert!(malformed.get("result").is_none());
    assert_eq!(fixture.read("document.luau"), source);
    assert_eq!(session.finish(), 0);
}

#[test]
fn tokens() {
    let fixture = Fixture::new("cli", "project");
    let source =
        "local banner = \"😀\"\nlocal count: number = 1\ncount += 2\nreturn count, banner\n";
    fixture.write("protocol.luau", source);
    let supported = capabilities();
    let legend = supported["textDocument"]["semanticTokens"]["tokenTypes"].clone();
    let mut session = connect(&fixture, &supported);
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let full = session.request(
        "textDocument/semanticTokens/full",
        &json!({"textDocument":{"uri":address}}),
    );
    let decoded = decode(source, &full["data"], &legend);
    assert!(
        decoded
            .iter()
            .any(|token| token["kind"] == "keyword" && token["text"] == "local")
    );
    assert!(decoded.iter().any(|token| token["kind"] == "string"
        && token["text"] == "\"😀\""
        && token["length"] == 4));
    assert!(decoded.iter().any(|token| {
        token["kind"] == "variable"
            && token["text"] == "count"
            && token["line"] == 2
            && token["modifiers"]
                .as_u64()
                .is_some_and(|mask| mask & 2 != 0)
    }));
    let unchanged = session.request(
        "textDocument/semanticTokens/full/delta",
        &json!({"textDocument":{"uri":address},"previousResultId":full["resultId"]}),
    );
    assert_eq!(unchanged["edits"], json!([]));
    let updated = source.replace("count", "totalcount");
    session.notify(
        "textDocument/didChange",
        &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":updated}]}),
    );
    let delta = session.request(
        "textDocument/semanticTokens/full/delta",
        &json!({"textDocument":{"uri":address},"previousResultId":full["resultId"]}),
    );
    let mut reconstructed = full["data"]
        .as_array()
        .expect("full relative tokens")
        .clone();
    let edits = delta["edits"].as_array().expect("token delta edits");
    assert_ne!(edits.as_slice(), [] as [Value; 0]);
    for edit in edits.iter().rev() {
        let start = usize::try_from(edit["start"].as_u64().expect("delta start"))
            .expect("bounded delta start");
        let deleted = usize::try_from(edit["deleteCount"].as_u64().expect("delta deletion"))
            .expect("bounded delta deletion");
        let replacement = edit["data"].as_array().cloned().unwrap_or_default();
        let _removed = reconstructed.splice(start..start + deleted, replacement);
    }
    let current = session.request(
        "textDocument/semanticTokens/full",
        &json!({"textDocument":{"uri":address}}),
    );
    assert_eq!(json!(reconstructed), current["data"]);
    let recovered = session.request(
        "textDocument/semanticTokens/full/delta",
        &json!({"textDocument":{"uri":address},"previousResultId":"foreign"}),
    );
    assert_eq!(recovered["data"], current["data"]);
    let ranged = session.request("textDocument/semanticTokens/range", &json!({"textDocument":{"uri":address},"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":26}}}));
    let selected = decode(&updated, &ranged["data"], &legend);
    assert!(selected.iter().all(|token| token["line"] == 1));
    assert!(selected.iter().any(|token| token["text"] == "totalcount"));
    assert!(ranged.get("resultId").is_none());
    snapshot(
        &fixture,
        "server",
        "tokens",
        &json!({"full":full,"decoded":decoded,"unchanged":unchanged,"current":current,"range":ranged}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn structure() {
    let fixture = Fixture::new("cli", "structure");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");
    let folds = session.request(
        "textDocument/foldingRange",
        &json!({"textDocument":{"uri":address}}),
    );
    assert!(
        folds
            .as_array()
            .expect("fold regions")
            .iter()
            .any(|fold| fold["startLine"] == 8 && fold["endLine"] == 12)
    );
    assert!(
        folds
            .as_array()
            .expect("fold regions")
            .iter()
            .any(|fold| fold["startLine"] == 0
                && fold["endLine"] == 1
                && fold["kind"] == "comment")
    );
    assert!(folds.as_array().expect("line folds").iter().all(|fold| {
        fold.get("startCharacter").is_none()
            && fold.get("endCharacter").is_none()
            && fold["startLine"].as_u64().expect("fold start")
                < fold["endLine"].as_u64().expect("fold end")
    }));
    let selections = session.request("textDocument/selectionRange", &json!({"textDocument":{"uri":address},"positions":[{"line":10,"character":17},{"line":3,"character":5}]}));
    assert_eq!(selections.as_array().expect("selection chains").len(), 2);
    assert_eq!(
        selections[0]["range"],
        json!({"start":{"line":10,"character":15},"end":{"line":10,"character":21}})
    );
    for chain in selections.as_array().expect("selection chains") {
        let mut child = chain;
        let mut depth = 1;
        while let Some(parent) = child.get("parent") {
            let start = |value: &Value| {
                (
                    value["start"]["line"].as_u64().expect("start line"),
                    value["start"]["character"]
                        .as_u64()
                        .expect("start character"),
                )
            };
            let end = |value: &Value| {
                (
                    value["end"]["line"].as_u64().expect("end line"),
                    value["end"]["character"].as_u64().expect("end character"),
                )
            };
            assert!(start(&parent["range"]) <= start(&child["range"]));
            assert!(end(&parent["range"]) >= end(&child["range"]));
            assert_ne!(parent["range"], child["range"]);
            child = parent;
            depth += 1;
        }
        assert!(depth >= 3);
    }
    snapshot(
        &fixture,
        "server",
        "structure",
        &json!({"folds":folds,"selections":selections}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn colors() {
    let fixture = Fixture::new("cli", "server");
    fixture.write("instar.toml", "[environment]\nroblox = true\n");
    let source = "local first = Color3.fromRGB(255, 0, 128)\nlocal ignored = Color3.fromRGB(256, 0, 0)\ndo\n    local Color3 = { fromRGB = function(...) return nil end }\n    local shadowed = Color3.fromRGB(255, 0, 0)\nend\nreturn first, ignored\n";
    fixture.write("document.luau", source);
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "document.luau");
    let address = fixture.uri("document.luau");
    let colors = session.request(
        "textDocument/documentColor",
        &json!({"textDocument":{"uri":address}}),
    );
    assert_eq!(
        colors
            .as_array()
            .expect("supported unshadowed colors")
            .len(),
        1
    );
    for (component, expected) in [
        ("red", 1.0),
        ("green", 0.0),
        ("blue", 128.0 / 255.0),
        ("alpha", 1.0),
    ] {
        let actual = colors[0]["color"][component]
            .as_f64()
            .expect("numeric color component");
        assert!(
            (actual - expected).abs() <= f64::from(f32::EPSILON),
            "{component}: {actual}"
        );
    }
    assert_eq!(
        colors[0]["range"],
        json!({"start":{"line":0,"character":14},"end":{"line":0,"character":41}})
    );
    let representations = session.request("textDocument/colorPresentation", &json!({"textDocument":{"uri":address},"range":colors[0]["range"],"color":{"red":0,"green":1,"blue":0,"alpha":1}}));
    let entries = representations.as_array().expect("color representations");
    assert_eq!(entries.len(), 2);
    for label in ["Color3.new(0, 1, 0)", "Color3.fromRGB(0, 255, 0)"] {
        let presentation = entries
            .iter()
            .find(|entry| entry["label"] == label)
            .expect("equivalent RGB representation");
        assert_eq!(
            apply(source, &json!([presentation["textEdit"]]), "utf-16"),
            source.replace("Color3.fromRGB(255, 0, 128)", label)
        );
    }
    let invalid = session.response("textDocument/colorPresentation", &json!({"textDocument":{"uri":address},"range":colors[0]["range"],"color":{"red":2,"green":1,"blue":0,"alpha":1}}));
    assert_eq!(invalid["error"]["code"], -32602);
    assert_eq!(fixture.read("document.luau"), source);
    assert_eq!(session.finish(), 0);
}

fn decode(source: &str, data: &Value, legend: &Value) -> Vec<Value> {
    let data = data.as_array().expect("relative token data");
    assert_eq!(data.len() % 5, 0);
    let mut line = 0;
    let mut character = 0;
    let mut previous = (0, 0);
    data.as_chunks::<5>().0.iter().map(|token| {
        let next = usize::try_from(token[0].as_u64().expect("token line delta")).expect("bounded token line");
        line += next;
        if next != 0 {
            character = 0;
        }
        character += usize::try_from(token[1].as_u64().expect("token character delta")).expect("bounded token character");
        let length = usize::try_from(token[2].as_u64().expect("token length")).expect("bounded token length");
        assert!(length > 0);
        assert!((line, character) >= previous, "semantic tokens overlap");
        let units = source.lines().nth(line).expect("token source line").encode_utf16().collect::<Vec<_>>();
        let text = String::from_utf16(&units[character..character + length]).expect("semantic token boundaries");
        let kind = usize::try_from(token[3].as_u64().expect("token kind")).expect("bounded token kind");
        let kind = legend.as_array().expect("token legend").get(kind).expect("negotiated token kind");
        let modifiers = token[4].as_u64().expect("token modifiers");
        assert!(modifiers < 16, "unnegotiated token modifiers");
        previous = (line, character + length);
        json!({"line":line,"character":character,"length":length,"kind":kind,"modifiers":modifiers,"text":text})
    }).collect()
}
