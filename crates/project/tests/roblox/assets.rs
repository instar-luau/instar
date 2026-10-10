use std::{
    collections::BTreeMap,
    fs, io,
    path::Path,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use instar_shared::{Options, SourceIdentity, SourceSnapshot, SourceVersion};

use crate::{
    ChangeBatch, InputIdentity, InputState, Project, ProjectChange, Security, SourceChange,
    SourceOrigin, acquisition,
    assets::{load, publish, verify_bundle},
    capture::Capture,
    roblox::members::Members,
};

fn options() -> Options {
    Options::new(Some(Instant::now() + Duration::from_secs(5)))
}

fn files(name: &str) -> io::Result<BTreeMap<String, String>> {
    fs::read_dir(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/assets")
            .join(name),
    )?
    .map(|entry| {
        let entry = entry?;

        Ok((
            entry.file_name().into_string().expect("fixture filename"),
            fs::read_to_string(entry.path())?,
        ))
    })
    .collect()
}

#[test]
fn member_precedence_requires_known_inheritance() -> io::Result<()> {
    let source = SourceSnapshot::new(
        SourceVersion {
            identity: SourceIdentity::new(1),
            revision: 1,
        },
        "declare extern type Base with Name: string end\n\
        declare extern type Child extends Base with end\n\
        declare extern type Orphan extends Missing with Own: string end",
    )?;

    let members = Members::parse(&source, &options())?;
    assert!(members.contains("Base", "Name")?);
    assert!(!members.contains("Base", "Absent")?);
    assert!(members.contains("Child", "Name")?);
    assert!(!members.contains("Child", "Absent")?);
    assert!(members.contains("Orphan", "Own")?);

    for class in ["Missing", "Orphan"] {
        let error = members
            .contains(class, "Absent")
            .expect_err("unknown precedence");

        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert!(error.to_string().contains("Missing"));
    }

    Ok(())
}

#[test]
fn verifies_profiles_and_loads_cache() -> io::Result<()> {
    let expected = files("original")?;
    let verified = verify_bundle("roblox", expected.clone(), &options())?;
    assert_eq!(verified.files(), &expected);
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");
    publish(&cache, verified, &options())?;
    let project = Project::new(Vec::new());

    for (profile, security) in [
        ("none", Security::None),
        ("local", Security::Local),
        ("plugin", Security::Plugin),
        ("roblox", Security::Roblox),
    ] {
        let mut original = Capture::live(Arc::clone(&project.host));
        let assets = load(&cache, security, &mut original, &options())?;
        let guarded = original.guards();

        project.change(ChangeBatch {
            changes: vec![ProjectChange::Source(SourceChange::Open {
                identity: project.source_identity(),
                origin: SourceOrigin::Disk(directory.path().join("unrelated.luau")),
                text: Arc::from("return 1"),
            })],
        })?;

        assert!(project.current(&guarded));
        let mut repeated = Capture::live(Arc::clone(&project.host));
        let reloaded = load(&cache, security, &mut repeated, &options())?;

        assert_eq!(
            assets.definitions[0].version,
            reloaded.definitions[0].version
        );

        assert_eq!(guarded, repeated.guards());

        let metadata = project
            .snapshot()
            .input_versions
            .get(&InputIdentity::Path(cache.join("metadata.json")))
            .expect("metadata guard")
            .clone();

        let mut invalidated = metadata.clone();
        invalidated.revision += 1;
        invalidated.state = InputState::Stale;

        project.change(ChangeBatch {
            changes: vec![ProjectChange::Input {
                expected: Some(metadata),
                current: invalidated,
            }],
        })?;

        assert!(!project.current(&guarded));
        let mut refreshed = Capture::live(Arc::clone(&project.host));
        let reloaded = load(&cache, security, &mut refreshed, &options())?;
        assert!(project.current(&refreshed.guards()));

        assert_eq!(
            reloaded.definitions[0].text.as_ref(),
            expected[&format!("{profile}.d.luau")]
        );

        let declarations = SourceSnapshot::new(
            SourceVersion {
                identity: project.source_identity(),
                revision: 1,
            },
            expected[&format!("{profile}.d.luau")].clone(),
        )?;

        Members::parse(&declarations, &options())?;
    }

    directory.close()
}

#[test]
fn cold_publishers_preserve_the_first_bundle_and_reader_snapshot() -> io::Result<()> {
    let expected = files("original")?;
    let replacement = verify_bundle("roblox", files("replacement")?, &options())?;
    let original = verify_bundle("roblox", expected.clone(), &options())?;
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");
    let metadata = cache.join("metadata.json");
    let (ready, prepared) = mpsc::sync_channel(1);
    let (publishable, release) = mpsc::sync_channel(1);

    thread::scope(|scope| -> io::Result<()> {
        let target = cache.clone();
        let manifest = metadata.clone();

        let publisher = scope.spawn(move || -> io::Result<()> {
            let project = Project::new(Vec::new());
            let mut capture = Capture::live(Arc::clone(&project.host));

            assert_eq!(
                capture
                    .source(&manifest, &options())
                    .expect_err("cold cache")
                    .kind(),
                io::ErrorKind::NotFound
            );

            ready.send(()).expect("publisher ready");

            release
                .recv_timeout(Duration::from_secs(5))
                .expect("release publisher");

            publish(&target, replacement, &options())?;
            let mut capture = Capture::live(Arc::clone(&project.host));
            load(&target, Security::None, &mut capture, &options())?;

            Ok(())
        });

        let project = Project::new(Vec::new());
        let mut capture = Capture::live(Arc::clone(&project.host));

        assert_eq!(
            capture
                .source(&metadata, &options())
                .expect_err("cold cache")
                .kind(),
            io::ErrorKind::NotFound
        );

        prepared
            .recv_timeout(Duration::from_secs(5))
            .expect("both publishers cold");

        publish(&cache, original, &options())?;
        let mut capture = Capture::live(Arc::clone(&project.host));
        let retained = capture.source(&metadata, &options())?;
        let snapshot = capture.snapshot.clone();
        publishable.send(()).expect("release publisher");
        publisher.join().expect("asset publisher")?;

        assert_eq!(
            snapshot
                .source(retained.version.identity)
                .expect("reader snapshot"),
            &retained
        );

        for (profile, security) in [
            ("none", Security::None),
            ("local", Security::Local),
            ("plugin", Security::Plugin),
            ("roblox", Security::Roblox),
        ] {
            let mut capture = Capture::live(Arc::clone(&project.host));
            let assets = load(&cache, security, &mut capture, &options())?;

            assert_eq!(
                assets.definitions[0].text.as_ref(),
                expected[&format!("{profile}.d.luau")]
            );
        }

        for (name, text) in expected {
            assert_eq!(fs::read_to_string(cache.join(name))?, text);
        }

        assert_eq!(fs::read_dir(directory.path())?.count(), 1);

        Ok(())
    })?;

    directory.close()
}

#[test]
fn failed_publication_leaves_no_partial_bundle() -> io::Result<()> {
    let directory = tempfile::tempdir()?;
    let cache = directory.path().join("bundle");
    fs::create_dir_all(cache.join("plugin.d.luau"))?;
    let bundle = verify_bundle("roblox", files("original")?, &options())?;
    assert!(publish(&cache, bundle, &options()).is_err());
    assert_eq!(fs::read_dir(&cache)?.count(), 1);
    assert_eq!(fs::read_dir(directory.path())?.count(), 1);

    directory.close()
}

#[test]
fn rejects_corrupt_bundles_and_insecure_selection() -> io::Result<()> {
    let mut corrupted = files("original")?;
    corrupted.insert("none.d.luau".to_owned(), "declare forged: any".to_owned());

    assert_eq!(
        verify_bundle("roblox", corrupted, &options())
            .expect_err("hash mismatch")
            .kind(),
        io::ErrorKind::InvalidData
    );

    assert!(verify_bundle("roblox", BTreeMap::new(), &options()).is_err());
    assert!(acquisition::validate("http://127.0.0.1:1/metadata.json", &"0".repeat(64)).is_err());

    Ok(())
}

#[test]
fn bundle_verification_obeys_deadline_and_cancellation() -> io::Result<()> {
    let started = Instant::now();
    let expired = Options::new(Some(started));

    assert_eq!(
        verify_bundle("roblox", files("original")?, &expired)
            .expect_err("deadline")
            .kind(),
        io::ErrorKind::TimedOut
    );

    assert!(started.elapsed() < Duration::from_millis(250));
    let options = options();
    options.cancellation.cancel();

    assert_eq!(
        verify_bundle("roblox", files("original")?, &options)
            .expect_err("cancellation")
            .kind(),
        io::ErrorKind::Interrupted
    );

    Ok(())
}
