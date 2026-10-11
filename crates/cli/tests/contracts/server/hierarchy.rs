use serde_json::json;

use super::{
    super::support::{Fixture, snapshot},
    connection::{capabilities, connect, open},
};

#[test]
fn calls() {
    let fixture = Fixture::new("cli", "hierarchy");
    let mut session = connect(&fixture, &capabilities());
    let source = open(&mut session, &fixture, "input.luau");
    let address = fixture.uri("input.luau");
    let functions = session.request(
        "textDocument/prepareCallHierarchy",
        &json!({"textDocument":{"uri":address},"position":{"line":5,"character":5}}),
    );
    assert_eq!(
        functions.as_array().expect("alias target hierarchy").len(),
        1
    );
    let target = &functions[0];
    assert_eq!(target["name"], "target");
    assert_eq!(target["uri"], fixture.uri("dependency.luau"));
    assert_eq!(
        target["selectionRange"],
        json!({"start":{"line":0,"character":15},"end":{"line":0,"character":21}})
    );
    let incoming = session.request("callHierarchy/incomingCalls", &json!({"item":target}));
    let callers = incoming.as_array().expect("incoming calls");
    let caller = callers
        .iter()
        .find(|call| call["from"]["name"] == "caller")
        .expect("real caller");
    let unicode = source.lines().nth(4).expect("unicode call");
    let offset = unicode[..unicode.find("alias").expect("alias call")]
        .encode_utf16()
        .count();
    assert_eq!(
        caller["fromRanges"],
        json!([
            {"start":{"line":4,"character":offset},"end":{"line":4,"character":offset+5}},
            {"start":{"line":5,"character":4},"end":{"line":5,"character":9}}
        ])
    );
    assert!(callers.iter().any(|call| {
        call["from"]["name"] == "nested"
            && call["fromRanges"]
                .as_array()
                .is_some_and(|ranges| ranges.len() == 1)
    }));
    assert!(callers.iter().any(|call| call["from"]["name"] == "consumer"
        && call["from"]["uri"] == fixture.uri("consumer.luau")));
    assert!(!callers.iter().any(|call| call["from"]["name"] == "shadow"));
    let outgoing = session.request(
        "callHierarchy/outgoingCalls",
        &json!({"item":caller["from"]}),
    );
    let targets = outgoing.as_array().expect("outgoing calls");
    assert_eq!(targets.len(), 2);
    assert!(
        targets.iter().any(
            |call| call["to"]["name"] == "target" && call["fromRanges"] == caller["fromRanges"]
        )
    );
    assert!(targets.iter().any(|call| call["to"]["name"] == "nested"
        && call["fromRanges"]
            == json!([{ "start":{"line":9,"character":4},"end":{"line":9,"character":10}}])));
    let mut forged = target.clone();
    forged["name"] = json!("different");
    assert_eq!(
        session.response("callHierarchy/incomingCalls", &json!({"item":forged}))["error"]["code"],
        -32602
    );
    session.notify("textDocument/didChange", &json!({"textDocument":{"uri":address,"version":2},"contentChanges":[{"text":"return 1\n"}]}));
    assert_eq!(
        session.response(
            "callHierarchy/outgoingCalls",
            &json!({"item":caller["from"]})
        )["error"]["code"],
        -32801
    );
    snapshot(
        &fixture,
        "server",
        "calls",
        &json!({"prepared":functions,"incoming":incoming,"outgoing":outgoing}),
    );
    assert_eq!(session.finish(), 0);
}

#[test]
fn nominals() {
    let fixture = Fixture::new("cli", "hierarchy");
    let mut session = connect(&fixture, &capabilities());
    open(&mut session, &fixture, "types.luau");
    let address = fixture.uri("types.luau");
    let prepared = session.request(
        "textDocument/prepareTypeHierarchy",
        &json!({"textDocument":{"uri":address},"position":{"line":0,"character":14}}),
    );
    assert_eq!(prepared.as_array().expect("nominal selection").len(), 1);
    let base = &prepared[0];
    assert_eq!(base["name"], "Base");
    assert_eq!(base["kind"], 5);
    assert_eq!(
        base["selectionRange"],
        json!({"start":{"line":0,"character":20},"end":{"line":0,"character":24}})
    );
    let declaration = base["uri"].as_str().expect("nominal declaration address");
    assert!(declaration.starts_with("instar://definition/"));
    assert_eq!(
        session.request("workspace/textDocumentContent", &json!({"uri":declaration}))["text"],
        fixture.read("definitions.luau")
    );
    assert_eq!(
        session.response(
            "textDocument/formatting",
            &json!({"textDocument":{"uri":declaration},"options":{"tabSize":2,"insertSpaces":true}})
        )["error"]["code"],
        -32602
    );
    assert_eq!(
        session.request("typeHierarchy/supertypes", &json!({"item":base})),
        json!([])
    );
    let children = session.request("typeHierarchy/subtypes", &json!({"item":base}));
    assert_eq!(children.as_array().expect("direct descendants").len(), 1);
    assert_eq!(children[0]["name"], "Child");
    let parents = session.request("typeHierarchy/supertypes", &json!({"item":children[0]}));
    assert_eq!(parents.as_array().expect("direct parents").len(), 1);
    assert_eq!(parents[0]["name"], "Base");
    let descendants = session.request("typeHierarchy/subtypes", &json!({"item":children[0]}));
    assert_eq!(descendants.as_array().expect("grandchild").len(), 1);
    assert_eq!(descendants[0]["name"], "Grandchild");
    let structural = session.request(
        "textDocument/prepareTypeHierarchy",
        &json!({"textDocument":{"uri":address},"position":{"line":2,"character":24}}),
    );
    assert_eq!(structural, json!([]));
    fixture.write(
        "definitions.luau",
        fixture
            .read("definitions.luau")
            .replace("child: string", "child: number"),
    );
    session.notify(
        "workspace/didChangeWatchedFiles",
        &json!({"changes":[{"uri":fixture.uri("definitions.luau"),"type":2}]}),
    );
    assert_eq!(
        session.response("typeHierarchy/subtypes", &json!({"item":base}))["error"]["code"],
        -32801
    );
    assert_eq!(
        session.request("workspace/textDocumentContent", &json!({"uri":declaration}))["text"],
        fixture
            .read("definitions.luau")
            .replace("child: number", "child: string")
    );
    snapshot(
        &fixture,
        "server",
        "nominals",
        &json!({"prepared":prepared,"children":children,"parents":parents,"descendants":descendants,"structural":structural}),
    );
    assert_eq!(session.finish(), 0);
}
