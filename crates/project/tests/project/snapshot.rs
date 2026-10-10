use std::{io, sync::Arc, thread};

use instar_project::{
    ChangeBatch, CheckOverrides, Input, InputIdentity, Invocation, OperationOverrides, Project,
    ProjectChange, Query, QueryOutput, QueryRequest, SettingsRequest, SourceChange, SourceOrigin,
};

use instar_shared::Completion;

use crate::support::{Directory, complete, options, replace};

#[test]
fn snapshots_preserve_overlays_across_threads_and_retain_unaffected_caches() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let path = directory.file(
        "entry.luau",
        include_str!("../fixtures/lifecycle/overlays/caller.luau"),
    )?;

    let project = Project::new(vec![directory.path().to_path_buf()]);
    replace(&project, &path, "return 2")?;

    let settings = complete(project.settings(
        SettingsRequest {
            snapshot: None,
            invocation: Invocation {
                inputs: vec![Input::Path(path.clone())],
                ..Invocation::default()
            },
            overrides: OperationOverrides::Check(CheckOverrides::default()),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(settings.completion, Completion::Complete);
    let original = settings.snapshot.clone();
    let source = original.source_at(&path).expect("source").clone();

    let consulted = original
        .input_versions
        .get(&InputIdentity::Source(source.version.identity))
        .expect("source guard")
        .clone();

    replace(&project, &directory.path().join("other.luau"), "return 3")?;
    assert!(project.current(&[consulted.clone()]));

    assert_eq!(
        project.snapshot().source_at(&path).expect("source").version,
        source.version
    );

    assert!(Arc::ptr_eq(
        &source.text,
        &project.snapshot().source_at(&path).expect("source").text
    ));

    let unchanged = complete(project.settings(
        SettingsRequest {
            snapshot: None,
            invocation: Invocation {
                inputs: vec![Input::Path(path.clone())],
                ..Invocation::default()
            },
            overrides: OperationOverrides::Check(CheckOverrides::default()),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        serde_json::to_value(&settings.settings).map_err(io::Error::other)?,
        serde_json::to_value(&unchanged.settings).map_err(io::Error::other)?
    );

    replace(&project, &path, "return 4")?;
    let previous = original.clone();
    let identity = source.version.identity;

    let old_source = thread::spawn(move || {
        previous
            .source(identity)
            .expect("historical source")
            .clone()
    })
    .join()
    .expect("snapshot worker");

    assert_eq!(old_source.text.as_ref(), "return 2");

    assert_eq!(
        project
            .snapshot()
            .source_at(&path)
            .expect("source")
            .text
            .as_ref(),
        "return 4"
    );

    assert!(!project.current(&[consulted]));
    assert!(project.snapshot().identity > original.identity);
    let expected = project.snapshot().source_at(&path).expect("source").version;

    project.change(ChangeBatch {
        changes: vec![ProjectChange::Source(SourceChange::Close { expected })],
    })?;

    let reloaded = complete(project.settings(
        SettingsRequest {
            snapshot: None,
            invocation: Invocation {
                inputs: vec![Input::Path(path.clone())],
                ..Invocation::default()
            },
            overrides: OperationOverrides::Check(CheckOverrides::default()),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        reloaded
            .snapshot
            .source_at(&path)
            .expect("disk source")
            .text
            .as_ref(),
        include_str!("../fixtures/lifecycle/overlays/caller.luau")
    );

    let historical = complete(project.query(
        QueryRequest {
            snapshot: original,
            operation: Query::Content(identity),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let QueryOutput::Content(Some(historical)) = historical.output else {
        panic!("historical content");
    };

    assert_eq!(historical.text.as_ref(), "return 2");

    Ok(())
}

#[test]
fn snapshots_invalidate_configuration_and_can_restore_earlier_inputs() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let path = directory.file("entry.luau", "return 1")?;
    let project = Project::new(vec![directory.path().to_path_buf()]);

    let invocation = Invocation {
        inputs: vec![Input::Path(path)],
        ..Invocation::default()
    };

    let settings = complete(project.settings(
        SettingsRequest {
            snapshot: None,
            invocation: invocation.clone(),
            overrides: OperationOverrides::Check(CheckOverrides::default()),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    let original = settings.snapshot.clone();
    replace(&project, &directory.path().join("instar.toml"), "[")?;
    assert!(!project.current(&settings.inputs));

    assert!(
        complete(project.settings(
            SettingsRequest {
                snapshot: None,
                invocation: invocation.clone(),
                overrides: OperationOverrides::Check(CheckOverrides::default()),
            },
            options()
        ))
        .is_err()
    );

    let restored = complete(project.settings(
        SettingsRequest {
            snapshot: Some(original),
            invocation,
            overrides: OperationOverrides::Check(CheckOverrides::default()),
        },
        options(),
    ))
    .map_err(io::Error::from)?;

    assert_eq!(
        settings.settings[0].settings.formatting.width,
        restored.settings[0].settings.formatting.width
    );

    assert_eq!(
        serde_json::to_value(&settings.settings).map_err(io::Error::other)?,
        serde_json::to_value(&restored.settings).map_err(io::Error::other)?
    );

    assert!(
        project
            .change(ChangeBatch {
                changes: vec![ProjectChange::Source(SourceChange::Open {
                    identity: project.source_identity(),
                    origin: SourceOrigin::Disk("relative.luau".into()),
                    text: Arc::from("return 1"),
                })]
            })
            .is_err()
    );

    Ok(())
}
