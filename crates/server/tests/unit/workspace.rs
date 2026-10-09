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

#[test]
fn virtual_documents_have_private_identity_and_read_only_content_is_protected() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let mut workspace = Workspace::default();
    workspace.roots.insert(directory.path().to_path_buf());

    for (address, editable) in [("untitled:example", true), ("instar:definition", false)] {
        let uri = Uri::parse(address).expect("virtual URI");
        workspace.open(serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}})).expect("open parameters")).expect("open virtual document");
        let (_, path, _) = workspace.capture(&uri).expect("virtual snapshot");
        assert!(path.starts_with(directory.path()));
        assert_eq!(workspace.locations().get(&path), Some(&uri));
        assert_eq!(workspace.content(&uri).expect("content"), "return 1");
        let changed = workspace.change(serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]})).expect("change parameters"));
        assert_eq!(changed.is_ok(), editable);

        assert_eq!(
            workspace.content(&uri).expect("content"),
            if editable { "return 2" } else { "return 1" }
        );

        assert!(
            workspace
                .save(
                    serde_json::from_value(json!({"textDocument":{"uri":uri}}))
                        .expect("save parameters")
                )
                .is_err()
        );

        workspace
            .close(
                serde_json::from_value(json!({"textDocument":{"uri":uri}}))
                    .expect("close parameters"),
            )
            .expect("close");

        assert!(workspace.capture(&uri).is_err());
        assert!(!path.parent().expect("virtual directory").exists());
    }

    assert!(workspace.open(serde_json::from_value(json!({"textDocument":{"uri":"https://example.invalid/source.luau","languageId":"luau","version":1,"text":"return 1"}})).expect("unsupported URI parameters")).is_err());
}

#[test]
fn rename_preserves_open_overlays_and_rejects_collisions() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let source = directory.path().join("source.luau");
    let target = directory.path().join("target.luau");
    let original = Uri::from_file_path(&source).expect("source URI");
    let renamed = Uri::from_file_path(&target).expect("target URI");
    let mut workspace = Workspace::default();
    workspace.open(serde_json::from_value(json!({"textDocument":{"uri":original,"languageId":"luau","version":7,"text":"return 1"}})).expect("open parameters")).expect("open");
    let previous = workspace.snapshot.clone();
    workspace.renamed(&original, &renamed).expect("rename");

    assert_eq!(
        workspace.content(&renamed).expect("renamed overlay"),
        "return 1"
    );

    assert_eq!(workspace.documents().get(&target), Some(&7));
    assert!(workspace.snapshot.source(&source).is_none());
    assert!(!workspace.current(&previous, &BTreeSet::from([source.clone()])));
    workspace.open(serde_json::from_value(json!({"textDocument":{"uri":original,"languageId":"luau","version":1,"text":"return 2"}})).expect("open parameters")).expect("open source");
    assert!(workspace.renamed(&original, &renamed).is_err());

    assert_eq!(
        workspace.content(&original).expect("source preserved"),
        "return 2"
    );

    assert_eq!(
        workspace.content(&renamed).expect("target preserved"),
        "return 1"
    );
}
