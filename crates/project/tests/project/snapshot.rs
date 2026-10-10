use std::{collections::BTreeSet, io, rc::Rc, thread, time::Duration};

use instar_core::{
    project::{Change, Project},
    snapshot::Snapshot,
};

use crate::support::Directory;

#[test]
fn snapshots_preserve_overlays_across_threads_and_retain_unaffected_caches() -> io::Result<()> {
    let directory = Directory::new(None)?;

    let path = directory.file(
        "entry.luau",
        include_str!("../fixtures/lifecycle/overlays/caller.luau"),
    )?;

    let mut snapshot = Snapshot::default();

    snapshot.change(Change::Overlay {
        path: path.clone(),
        text: Some("return 2".to_owned()),
    })?;

    let original = snapshot.clone();
    let mut project = Project::new(Duration::from_secs(2));
    project.apply(&snapshot);
    let source = project.source(&path)?;
    let settings = project.configuration(directory.path())?;

    snapshot.change(Change::Overlay {
        path: directory.path().join("other.luau"),
        text: Some("return 3".to_owned()),
    })?;

    assert!(snapshot.current(&original, &BTreeSet::from([path.clone()])));

    project.apply(&snapshot);
    assert_eq!(project.source(&path)?.revision, source.revision);

    assert!(Rc::ptr_eq(
        &settings,
        &project.configuration(directory.path())?
    ));

    snapshot.change(Change::Overlay {
        path: path.clone(),
        text: Some("return 4".to_owned()),
    })?;

    let copy = path.clone();
    let previous = original.clone();

    let old_source = thread::spawn(move || {
        let mut project = Project::new(Duration::from_secs(2));
        project.apply(&previous);

        project.source(&copy)
    })
    .join()
    .expect("snapshot worker")?;

    assert_eq!(old_source.text.as_ref(), "return 2");
    project.apply(&snapshot);
    assert_eq!(project.source(&path)?.text.as_ref(), "return 4");
    assert!(!snapshot.current(&original, &BTreeSet::from([path.clone()])));
    assert!(snapshot.revision() > original.revision());
    snapshot.change(Change::Close(path.clone()))?;
    project.apply(&snapshot);

    assert_eq!(
        project.source(&path)?.text.as_ref(),
        include_str!("../fixtures/lifecycle/overlays/caller.luau")
    );

    Ok(())
}

#[test]
fn snapshots_invalidate_configuration_and_can_restore_earlier_inputs() -> io::Result<()> {
    let directory = Directory::new(None)?;
    let mut snapshot = Snapshot::default();
    let original = snapshot.clone();
    let mut project = Project::new(Duration::from_secs(2));
    let settings = project.configuration(directory.path())?;

    snapshot.change(Change::Overlay {
        path: directory.path().join("instar.toml"),
        text: Some("[".to_owned()),
    })?;

    project.apply(&snapshot);
    assert!(project.configuration(directory.path()).is_err());
    project.apply(&original);
    let restored = project.configuration(directory.path())?;
    assert!(!Rc::ptr_eq(&settings, &restored));

    assert_eq!(
        settings.configuration.format.width,
        restored.configuration.format.width
    );

    assert!(
        snapshot
            .change(Change::Disk("relative.luau".into()))
            .is_err()
    );

    Ok(())
}
