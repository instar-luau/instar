use std::collections::BTreeSet;

use serde_json::{Value, json};
use tower_lsp_server::gen_lsp_types::Uri;

use crate::workspace::Workspace;

#[test]
fn edits_are_atomic_versioned_and_require_recovery_after_invalid_ranges() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");

    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/edits.json")).expect("edit fixture");

    let mut workspace = Workspace::default();
    workspace.open(serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":fixture["source"]}})).expect("open parameters")).expect("open");
    let (original, path, _) = workspace.capture(&uri).expect("original snapshot");

    workspace
        .change(
            serde_json::from_value(
                json!({"textDocument":{"uri":uri,"version":2},"contentChanges":fixture["changes"]}),
            )
            .expect("change parameters"),
        )
        .expect("change");

    assert_eq!(
        workspace.snapshot.source(&path).expect("overlay").as_ref(),
        fixture["expected"].as_str().expect("expected text")
    );

    assert_eq!(
        original.source(&path).expect("original overlay").as_ref(),
        fixture["source"].as_str().expect("source text")
    );

    let valid = workspace.snapshot.clone();
    assert!(workspace.change(serde_json::from_value(json!({"textDocument":{"uri":uri,"version":3},"contentChanges":fixture["invalid"]})).expect("invalid parameters")).is_err());
    assert_eq!(workspace.snapshot.source(&path), valid.source(&path));
    assert!(workspace.capture(&uri).is_err());
    assert!(!workspace.current(&valid, &BTreeSet::from([path.clone()])));
    assert!(workspace.change(serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"old"}]})).expect("stale parameters")).is_err());
    workspace.change(serde_json::from_value(json!({"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":fixture["source"]}]})).expect("recovery parameters")).expect("recover");
    assert!(workspace.capture(&uri).is_ok());

    workspace
        .save(serde_json::from_value(json!({"textDocument":{"uri":uri}})).expect("save parameters"))
        .expect("save");

    assert_eq!(workspace.snapshot.source(&path), original.source(&path));

    workspace
        .close(
            serde_json::from_value(json!({"textDocument":{"uri":uri}})).expect("close parameters"),
        )
        .expect("close");

    assert!(workspace.snapshot.source(&path).is_none());

    assert!(
        workspace
            .capture(&Uri::parse("untitled:example").expect("virtual URI"))
            .is_err()
    );
}
