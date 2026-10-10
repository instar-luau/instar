use std::{fs, sync::Arc};

use instar_project::{InputIdentity, Project, ProjectSnapshot, SourceOrigin};
use instar_shared::{Options, SourceSnapshot, SourceVersion};
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
    let options = Options::default();
    state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":fixture["source"]}})).expect("open parameters")).expect("open");

    let (original, source) = state
        .capture(&project, &uri, &options)
        .expect("original snapshot");

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
    assert!(state.capture(&project, &uri, &options).is_err());
    assert!(!state.current(&project, &inputs));
    assert!(state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":2},"contentChanges":[{"text":"old"}]})).expect("stale parameters")).is_err());
    state.change(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"version":4},"contentChanges":[{"text":fixture["source"]}]})).expect("recovery parameters")).expect("recover");
    assert!(state.capture(&project, &uri, &options).is_ok());

    state
        .save(
            &project,
            serde_json::from_value(json!({"textDocument":{"uri":uri}})).expect("save parameters"),
            &options,
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
                &Uri::parse("untitled:example").expect("virtual URI"),
                &options,
            )
            .is_err()
    );
}

#[test]
fn virtual_documents_have_private_identity_and_read_only_content_is_protected() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let project = Project::new(vec![directory.path().to_path_buf()]);
    let mut state = State::new(1);
    let options = Options::default();

    state
        .workspace_folders
        .insert(directory.path().to_path_buf());

    let mut identities = Vec::new();
    let mut contexts = Vec::new();

    for (address, editable) in [("untitled:example", true), ("instar:definition", false)] {
        let uri = Uri::parse(address).expect("virtual URI");
        state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":uri,"languageId":"luau","version":1,"text":"return 1"}})).expect("open parameters")).expect("open virtual document");

        let (snapshot, source) = state
            .capture(&project, &uri, &options)
            .expect("virtual snapshot");

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
                .capture(&project, &uri, &options)
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
                        .expect("save parameters"),
                    &options,
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

        assert!(state.capture(&project, &uri, &options).is_err());
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
    let options = Options::default();
    state.open(&project,serde_json::from_value(json!({"textDocument":{"uri":original,"languageId":"luau","version":7,"text":"return 1"}})).expect("open parameters")).expect("open");
    let previous = project.snapshot();
    let identity = state.documents[&original].source;

    state
        .renamed(&project, &[(source.clone(), target.clone())], Vec::new())
        .expect("rename");

    assert_eq!(
        state
            .capture(&project, &renamed, &options)
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
            .capture(&project, &original, &options)
            .expect("source preserved")
            .1
            .text
            .as_ref(),
        "return 2"
    );

    assert_eq!(
        state
            .capture(&project, &renamed, &options)
            .expect("target preserved")
            .1
            .text
            .as_ref(),
        "return 1"
    );
}

#[test]
fn declarations_retain_exact_history_without_live_registration_and_bound_closed_records() {
    let directory = tempfile::tempdir().expect("workspace directory");
    let path = directory.path().join("types.luau");
    let project = Project::new(Vec::new());
    let options = Options::default();
    let mut state = State::new(1);
    let unrelated = Uri::parse("untitled:unrelated").expect("unrelated URI");
    state.open(&project, serde_json::from_value(json!({"textDocument":{"uri":unrelated,"languageId":"luau","version":1,"text":"return 1"}})).expect("open parameters")).expect("open unrelated source");
    let live = project.snapshot();
    let unrelated_identity = InputIdentity::Source(state.documents[&unrelated].source);
    let identity = project.source_identity();
    let origin = SourceOrigin::Disk(path.clone());

    let publish = |state: &mut State, revision| {
        let source = SourceSnapshot::new(
            SourceVersion { identity, revision },
            format!("\u{feff}declare value{revision}: string\r\n"),
        )
        .expect("declaration source");

        let mut snapshot = ProjectSnapshot::declaration(source.clone(), origin.clone())
            .expect("declaration snapshot");

        Arc::make_mut(&mut snapshot.input_versions).insert(
            unrelated_identity.clone(),
            live.input_versions[&unrelated_identity].clone(),
        );

        let uri = state.uri(&snapshot, identity).expect("publish declaration");

        (uri, source)
    };

    let (pinned, original) = publish(&mut state, 1);
    fs::write(&path, "declare changed: number\n").expect("changed disk content");

    let (captured, source) = state
        .capture(&project, &pinned, &options)
        .expect("historical capture");

    assert_eq!(source, original);
    assert_eq!(captured.origin(identity), Some(&origin));
    assert!(captured.is_declaration(original.version));
    assert_eq!(captured.sources().count(), 1);
    assert!(!captured.input_versions.contains_key(&unrelated_identity));
    assert!(project.snapshot().source(identity).is_none());

    assert!(state.open(&project, serde_json::from_value(json!({"textDocument":{"uri":pinned,"languageId":"luau","version":1,"text":"replacement"}})).expect("mismatched parameters")).is_err());
    state.open(&project, serde_json::from_value(json!({"textDocument":{"uri":pinned,"languageId":"luau","version":1,"text":original.text}})).expect("definition parameters")).expect("pin declaration");
    assert!(!state.documents.contains_key(&pinned));
    assert!(state.editable(&pinned).is_err());
    assert!(state.change(&project, serde_json::from_value(json!({"textDocument":{"uri":pinned,"version":2},"contentChanges":[{"text":"replacement"}]})).expect("change parameters")).is_err());

    let (expired, _) = publish(&mut state, 2);

    for revision in 3..=258 {
        publish(&mut state, revision);
    }

    assert_eq!(state.declarations.len(), 257);

    assert_eq!(
        state
            .capture(&project, &pinned, &options)
            .expect("pinned history")
            .1,
        original
    );

    assert!(
        state
            .capture(&project, &expired, &options)
            .expect_err("expired capture")
            .message
            .contains("expired")
    );

    assert!(state.open(&project, serde_json::from_value(json!({"textDocument":{"uri":expired,"languageId":"luau","version":1,"text":"replacement"}})).expect("expired parameters")).expect_err("expired open").message.contains("expired"));
    assert_eq!(project.snapshot().sources().count(), 1);
    assert_eq!(state.documents.len(), 1);

    state
        .close(
            &project,
            serde_json::from_value(json!({"textDocument":{"uri":pinned}}))
                .expect("close parameters"),
        )
        .expect("close declaration");

    assert_eq!(state.declarations.len(), 256);

    for revision in 259..=514 {
        publish(&mut state, revision);
    }

    assert_eq!(state.declarations.len(), 256);

    assert!(
        state
            .capture(&project, &pinned, &options)
            .expect_err("closed history expires")
            .message
            .contains("expired")
    );

    assert_eq!(
        fs::read_to_string(path).expect("unchanged disk content"),
        "declare changed: number\n"
    );
}
