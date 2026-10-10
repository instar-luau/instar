use instar_project::Project;
use serde_json::{Value, json};
use tower_lsp_server::gen_lsp_types::Uri;

use crate::session::State;

#[test]
fn edits_are_atomic_versioned_and_require_recovery_after_invalid_ranges() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let uri = Uri::from_file_path(directory.path().join("entry.luau")).expect("file URI");

    let fixture: Value =
        serde_json::from_str(include_str!("../fixtures/edits.json")).expect("edit fixture");

    let project = Project::new(Vec::new());
    let mut state = State::new(1);
    state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":fixture["source"]}})).expect("open parameters")).expect("open");
    let (original, source) = state.capture(&project, &uri).expect("original snapshot");

    state
        .change(
            &project,
            serde_json::from_value(
                json!({"textDocument":{"uri":uri,"version":2},"contentChanges":fixture["changes"]}),
            )
            .expect("change parameters"),
        )
        .expect("change");

    assert_eq!(
        project
            .snapshot()
            .source(source.version.identity)
            .expect("overlay")
            .text
            .as_ref(),
        fixture["expected"].as_str().expect("expected text")
    );

    assert_eq!(
        original
            .source(source.version.identity)
            .expect("original overlay")
            .text
            .as_ref(),
        fixture["source"].as_str().expect("source text")
    );

    let valid = project.snapshot();
    let inputs = valid.input_versions.values().cloned().collect::<Vec<_>>();
    assert!(state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":3},"contentChanges":fixture["invalid"]})).expect("invalid parameters")).is_err());

    assert_eq!(
        project.snapshot().source(source.version.identity),
        valid.source(source.version.identity)
    );

    assert_eq!(state.documents[&uri].version, 2);
    assert!(state.capture(&project, &uri).is_err());
    assert!(!state.current(&project, &inputs));
    assert!(state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"old"}]})).expect("stale parameters")).is_err());
    state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":fixture["source"]}]})).expect("recovery parameters")).expect("recover");
    assert!(state.capture(&project, &uri).is_ok());

    state
        .save(
            &project,
            serde_json::from_value(json!({"textDocument":{"uri":uri}})).expect("save parameters"),
        )
        .expect("save");

    assert_eq!(
        project
            .snapshot()
            .source(source.version.identity)
            .expect("saved source")
            .text,
        original
            .source(source.version.identity)
            .expect("original source")
            .text
    );

    state
        .close(
            &project,
            serde_json::from_value(json!({"textDocument":{"uri":uri}})).expect("close parameters"),
        )
        .expect("close");

    assert!(project.snapshot().source(source.version.identity).is_none());

    assert!(
        state
            .capture(
                &project,
                &Uri::parse("untitled:example").expect("virtual URI")
            )
            .is_err()
    );
}

#[test]
fn virtual_documents_have_private_identity_and_read_only_content_is_protected() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let project = Project::new(vec![directory.path().to_path_buf()]);
    let mut state = State::new(1);

    state
        .workspace_folders
        .insert(directory.path().to_path_buf());

    let mut identities = Vec::new();
    let mut contexts = Vec::new();

    for (address, editable) in [("untitled:example", true), ("instar:definition", false)] {
        let uri = Uri::parse(address).expect("virtual URI");
        state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}})).expect("open parameters")).expect("open virtual document");
        let (snapshot, source) = state.capture(&project, &uri).expect("virtual snapshot");

        assert!(
            snapshot
                .origin(source.version.identity)
                .expect("virtual origin")
                .path()
                .expect("context path")
                .starts_with(directory.path())
        );

        assert_eq!(
            state
                .uri(&snapshot, source.version.identity)
                .expect("source URI"),
            uri
        );

        assert_eq!(source.text.as_ref(), "return 1");
        assert!(!identities.contains(&source.version.identity));
        identities.push(source.version.identity);
        let changed = state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"return 2"}]})).expect("change parameters"));
        assert_eq!(changed.is_ok(), editable);

        let context = snapshot
            .origin(source.version.identity)
            .expect("virtual origin")
            .path()
            .expect("context path");

        assert!(!contexts.iter().any(|previous| previous == context));
        contexts.push(context.to_path_buf());

        assert_eq!(
            state
                .capture(&project, &uri)
                .expect("current virtual source")
                .1
                .text
                .as_ref(),
            if editable { "return 2" } else { "return 1" }
        );

        assert!(
            state
                .save(
                    &project,
                    serde_json::from_value(json!({"textDocument":{"uri":uri}}))
                        .expect("save parameters")
                )
                .is_err()
        );

        state
            .close(
                &project,
                serde_json::from_value(json!({"textDocument":{"uri":uri}}))
                    .expect("close parameters"),
            )
            .expect("close");

        assert!(state.capture(&project, &uri).is_err());
        assert!(project.snapshot().source(source.version.identity).is_none());

        assert_eq!(
            directory
                .path()
                .read_dir()
                .expect("private context directory")
                .count(),
            0
        );
    }

    assert!(state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":"https://example.invalid/source.luau","languageId":"luau","version":1,"text":"return 1"}})).expect("unsupported URI parameters")).is_err());
}

#[test]
fn rename_preserves_open_overlays_and_rejects_collisions() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let source = directory.path().join("source.luau");
    let target = directory.path().join("target.luau");
    let original = Uri::from_file_path(&source).expect("source URI");
    let renamed = Uri::from_file_path(&target).expect("target URI");
    let project = Project::new(Vec::new());
    let mut state = State::new(1);
    state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":original,"languageId":"luau","version":7,"text":"return 1"}})).expect("open parameters")).expect("open");
    let previous = project.snapshot();
    let identity = state.documents[&original].source;

    state
        .renamed(&project, &[(source.clone(), target.clone())], Vec::new())
        .expect("rename");

    assert_eq!(
        state
            .capture(&project, &renamed)
            .expect("renamed overlay")
            .1
            .text
            .as_ref(),
        "return 1"
    );

    assert_eq!(state.documents[&renamed].version, 7);
    assert!(project.snapshot().source(identity).is_none());

    assert!(
        !state.current(
            &project,
            &previous
                .input_versions
                .values()
                .cloned()
                .collect::<Vec<_>>()
        )
    );

    state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":original,"languageId":"luau","version":1,"text":"return 2"}})).expect("open parameters")).expect("open source");

    assert!(
        state
            .renamed(&project, &[(source, target)], Vec::new())
            .is_err()
    );

    assert_eq!(
        state
            .capture(&project, &original)
            .expect("source preserved")
            .1
            .text
            .as_ref(),
        "return 2"
    );

    assert_eq!(
        state
            .capture(&project, &renamed)
            .expect("target preserved")
            .1
            .text
            .as_ref(),
        "return 1"
    );
}
