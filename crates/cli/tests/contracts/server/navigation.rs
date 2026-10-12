use serde_json::json;

use super::{
    super::support::{Fixture, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn bindings() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("protocol.luau", "local value: number = 1\nvalue += 2\nlocal function double(input: number): number\n    return input * 2\nend\nreturn double(value)\n");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");
    let position = json!({"textDocument":{"uri":address},"position":{"line":5,"character":14}});
    let target = json!([{"uri":address,"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":11}}}]);

    for method in ["textDocument/declaration", "textDocument/definition"] {
        assert_eq!(session.request(method, &position), target, "{method}");
    }

    let implementation = session.request(
        "textDocument/implementation",
        &json!({"textDocument":{"uri":address},"position":{"line":5,"character":8}}),
    );

    assert_eq!(
        implementation,
        json!([{"uri":address,"range":{"start":{"line":2,"character":15},"end":{"line":2,"character":21}}}])
    );

    let mut references = position.clone();
    references["context"] = json!({"includeDeclaration":true});
    let declared = session.request("textDocument/references", &references);

    assert_eq!(
        declared,
        json!([
            {"uri":address,"range":{"start":{"line":0,"character":6},"end":{"line":0,"character":11}}},
            {"uri":address,"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":5}}},
            {"uri":address,"range":{"start":{"line":5,"character":14},"end":{"line":5,"character":19}}}
        ])
    );

    references["context"]["includeDeclaration"] = json!(false);
    let used = session.request("textDocument/references", &references);
    assert_eq!(used, json!([declared[1], declared[2]]));
    references["partialResultToken"] = json!("references");

    assert_eq!(
        session.request("textDocument/references", &references),
        json!([])
    );

    let partial = session.message("$/progress")["params"].clone();
    assert_eq!(partial["token"], "references");
    assert_eq!(partial["value"], used);
    let highlights = session.request("textDocument/documentHighlight", &position);

    assert_eq!(
        highlights,
        json!([
            {"range":declared[0]["range"],"kind":3},
            {"range":declared[1]["range"],"kind":3},
            {"range":declared[2]["range"],"kind":2}
        ])
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn types() {
    let fixture = Fixture::new("cli", "navigation");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "input.luau");
    let definition = session.request("textDocument/typeDefinition", &json!({"textDocument":{"uri":fixture.uri("input.luau")},"position":{"line":2,"character":8}}));
    assert_eq!(definition.as_array().expect("type definitions").len(), 1);
    assert_eq!(definition[0]["uri"], fixture.uri("dependency.luau"));

    assert!(
        [
            json!({"start":{"line":0,"character":12},"end":{"line":0,"character":18}}),
            json!({"start":{"line":0,"character":21},"end":{"line":0,"character":36}}),
        ]
        .contains(&definition[0]["range"])
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn symbols() {
    let fixture = Fixture::new("cli", "project");
    fixture.write("protocol.luau", "local function outer()\n    local function inner()\n        return 1\n    end\n    return inner()\nend\nreturn outer()\n");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "protocol.luau");
    let address = fixture.uri("protocol.luau");

    let symbols = session.request(
        "textDocument/documentSymbol",
        &json!({"textDocument":{"uri":address}}),
    );

    let outer = symbols
        .as_array()
        .expect("document outline")
        .iter()
        .find(|symbol| symbol["name"] == "outer")
        .expect("outer symbol");

    assert_eq!(outer["kind"], 12);
    assert_eq!(outer["range"]["start"]["line"], 0);
    assert_eq!(outer["range"]["end"]["line"], 5);

    let inner = outer["children"]
        .as_array()
        .expect("nested outline")
        .iter()
        .find(|symbol| symbol["name"] == "inner")
        .expect("inner child");

    assert_eq!(inner["kind"], 12);

    assert_eq!(
        inner["selectionRange"],
        json!({"start":{"line":1,"character":19},"end":{"line":1,"character":24}})
    );

    let searched = session.request("workspace/symbol", &json!({"query":"outer"}));

    let selected = searched
        .as_array()
        .expect("workspace symbols")
        .iter()
        .find(|symbol| symbol["name"] == "outer" && symbol["location"]["uri"] == address)
        .expect("workspace outer");

    assert!(selected["location"].get("range").is_none());
    let resolved = session.request("workspaceSymbol/resolve", selected);
    assert_eq!(resolved["data"], selected["data"]);
    assert_eq!(resolved["location"]["range"], outer["selectionRange"]);
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));

    assert_eq!(
        session.response("workspaceSymbol/resolve", selected)["error"]["code"],
        -32801
    );

    assert_eq!(
        session.request("workspace/symbol", &json!({"query":"outer"})),
        json!([])
    );

    snapshot(
        &fixture,
        "server",
        "symbols",
        &json!({"outline":symbols,"workspace":searched,"resolved":resolved}),
    );

    assert_eq!(session.finish(), 0);
}

#[test]
fn links() {
    let fixture = Fixture::new("cli", "links");
    fixture.write("module.luau", "return {value = 1}\n");

    fixture.write(
        ".luaurc",
        "{\"languageMode\":\"strict\",\"lint\":{\"*\":false},\"aliases\":{\"modules\":\".\"}}",
    );

    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");

    let links = session.request(
        "textDocument/documentLink",
        &json!({"textDocument":{"uri":address}}),
    );

    let entries = links.as_array().expect("document links");

    let imports = entries
        .iter()
        .filter(|link| link["target"] == fixture.uri("module.luau"))
        .collect::<Vec<_>>();

    assert_eq!(imports.len(), 2);

    assert!(
        entries
            .iter()
            .any(|link| link["target"] == "https://example.com/reference_(value)")
    );

    assert!(!entries.iter().any(|link| {
        link["target"].as_str().is_some_and(|target| {
            target.contains("not-comment") || target.starts_with("javascript:")
        })
    }));

    assert!(
        !entries
            .iter()
            .any(|link| link["range"]["start"]["line"] == 5)
    );

    let link = imports[0];
    let resolved = session.request("documentLink/resolve", link);
    assert_eq!(resolved["target"], fixture.uri("module.luau"));
    assert_eq!(resolved["data"], link["data"]);
    assert_eq!(resolved["range"], link["range"]);
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));

    assert_eq!(
        session.response("documentLink/resolve", link)["error"]["code"],
        -32801
    );

    snapshot(
        &fixture,
        "server",
        "links",
        &json!({"links":links,"resolved":resolved}),
    );

    assert_eq!(session.finish(), 0);
}
