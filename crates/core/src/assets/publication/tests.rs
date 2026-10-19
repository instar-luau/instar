use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs::{self, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
};

use same_file::Handle;
use tempfile::TempDir;

use super::{
    Publication,
    transaction::{Installed, filesystem, stage},
};

use crate::{
    assets::{Error, Target},
    workspace::Replacement,
};

thread_local! {
    static CANCELLATION: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

struct Cancellation {
    remaining: usize,
    action: Option<Box<dyn FnOnce() -> bool>>,
}

struct Fixture {
    root: TempDir,
    directory: PathBuf,
    files: BTreeMap<PathBuf, String>,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().expect("create fixture directory");
        let directory = root.path().join("output");

        let files: BTreeMap<_, _> = Target::Roblox
            .files()
            .iter()
            .map(|relative| (PathBuf::from(relative), format!("new {relative}")))
            .collect();

        for relative in files.keys() {
            stage(
                &directory.join(relative),
                &format!("old {}", relative.display()),
                None,
            )
            .expect("seed original file");
        }

        Self {
            root,
            directory,
            files,
        }
    }

    fn publication(&self) -> Publication {
        Publication::new(&self.directory, &[Target::Roblox]).expect("capture original publication")
    }

    fn cancellation(&self, action: impl FnOnce() -> bool + 'static) {
        CANCELLATION.with(|state| {
            *state.borrow_mut() = Some(Cancellation {
                remaining: self.files.len() + 2,
                action: Some(Box::new(action)),
            });
        });
    }

    fn originals(&self) {
        for relative in self.files.keys() {
            assert_eq!(
                fs::read_to_string(self.directory.join(relative)).expect("read recovered file"),
                format!("old {}", relative.display()),
            );
        }
    }

    fn cleaned(&self) {
        let mut entries: Vec<_> = fs::read_dir(self.root.path())
            .expect("read fixture directory")
            .map(|entry| entry.expect("read fixture entry").file_name())
            .collect();

        entries.sort();
        assert_eq!(entries, ["output", "output.lock"]);
    }
}

fn cancelled() -> bool {
    CANCELLATION.with(|state| {
        let mut state = state.borrow_mut();
        let state = state.as_mut().expect("configure cancellation");

        if state.remaining == 0 {
            return true;
        }

        state.remaining -= 1;

        if state.remaining == 0 {
            state
                .action
                .take()
                .expect("retain final cancellation action")()
        } else {
            false
        }
    })
}

#[test]
fn cancellation_before_commit_preserves_originals() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    fixture.cancellation(|| true);

    assert!(matches!(
        publication.write(&fixture.files, cancelled),
        Err(Error::Cancelled)
    ));

    fixture.originals();
    fixture.cleaned();
}

#[test]
fn cancellation_after_precommit_is_deferred() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    fixture.cancellation(|| false);

    let generated = publication
        .write(&fixture.files, cancelled)
        .expect("finish publication after final cancellation checkpoint");

    assert_eq!(generated.changed, fixture.files.len());
    assert!(cancelled());

    for (relative, expected) in &fixture.files {
        assert_eq!(
            fs::read_to_string(fixture.directory.join(relative)).expect("read published file"),
            *expected,
        );
    }

    fixture.cleaned();
}

#[test]
fn late_failure_recovers_committed_files() {
    let fixture = Fixture::new();
    let publication = fixture.publication();
    let protected = fixture.directory.join("roblox/local.d.luau");

    let permissions = fs::metadata(&protected)
        .expect("read destination permissions")
        .permissions();

    let destination = protected.clone();
    let mut readonly = permissions.clone();
    readonly.set_readonly(true);

    fixture.cancellation(move || {
        fs::set_permissions(destination, readonly).expect("protect later destination");

        false
    });

    let result = publication.write(&fixture.files, cancelled);

    let retained = fs::metadata(&protected)
        .expect("read protected destination")
        .permissions()
        .readonly();

    fs::set_permissions(&protected, permissions).expect("restore fixture permissions");

    assert!(retained);
    assert!(matches!(result, Err(Error::Filesystem { path, .. }) if path == protected));
    fixture.originals();
    fixture.cleaned();
}

fn install(
    fixture: &Fixture,
    publication: &Publication,
    staging: &Path,
    relative: &str,
) -> Installed {
    let relative = PathBuf::from(relative);
    let proposed = &fixture.files[&relative];
    let original = publication.originals[&relative].as_ref();

    let path = if let Some(original) = original {
        let path = staging.join("original").join(&relative);

        stage(&path, &original.contents, Some(&original.permissions))
            .expect("stage original backup");

        path
    } else {
        let path = staging.join("proposed").join(&relative);
        stage(&path, proposed, None).expect("stage proposed file");

        path
    };

    let replacement = Replacement::new(&path, proposed).expect("prepare proposed file");
    let destination = fixture.directory.join(&relative);

    let result = if let Some(original) = original {
        replacement.commit(&destination, &original.contents, &original.identity)
    } else {
        replacement.create(&destination)
    };

    let identity = result.expect("install proposed file");

    let permissions = fs::metadata(&destination)
        .expect("read installed permissions")
        .permissions();

    Installed {
        relative,
        identity,
        permissions,
    }
}

#[test]
fn recovery_preserves_conflicts_and_continues() {
    enum Conflict {
        Contents,
        Permissions,
        Identity,
    }

    for absent in [false, true] {
        for conflict in [
            Conflict::Contents,
            Conflict::Permissions,
            Conflict::Identity,
        ] {
            let fixture = Fixture::new();

            let relatives = [
                "roblox/documentation.json",
                "roblox/local.d.luau",
                "roblox/none.d.luau",
            ];

            if absent {
                for relative in relatives {
                    fs::remove_file(fixture.directory.join(relative))
                        .expect("record initially absent destination");
                }
            }

            let publication = fixture.publication();

            let staging =
                tempfile::tempdir_in(fixture.root.path()).expect("create backup directory");

            let committed: Vec<_> = relatives
                .iter()
                .map(|relative| install(&fixture, &publication, staging.path(), relative))
                .collect();

            let destination = fixture.directory.join(relatives[2]);
            let permissions = committed[2].permissions.clone();
            let other = fixture.directory.join(relatives[0]);
            fs::write(&other, "another concurrent edit").expect("edit another installed file");

            match conflict {
                Conflict::Contents => fs::write(&destination, "concurrent edit")
                    .expect("edit installed file in place"),

                Conflict::Permissions => {
                    let mut readonly = permissions.clone();
                    readonly.set_readonly(true);
                    fs::set_permissions(&destination, readonly).expect("protect installed file");
                }

                Conflict::Identity => {
                    let replacement = fixture.root.path().join("replacement");

                    fs::write(&replacement, &fixture.files[Path::new(relatives[2])])
                        .expect("copy installed contents");

                    fs::set_permissions(&replacement, permissions.clone())
                        .expect("copy installed permissions");

                    fs::rename(replacement, &destination).expect("replace installed identity");
                }
            }

            let failure = fixture.directory.join("roblox/plugin.d.luau");

            let error = publication.recover(
                &fixture.files,
                staging,
                &committed,
                filesystem(&failure, io::Error::other("injected commit failure")),
            );

            let retained =
                fs::read_to_string(&destination).expect("retain conflicting destination");

            let readonly = fs::metadata(&destination)
                .expect("read conflicting permissions")
                .permissions()
                .readonly();

            let identity = Handle::from_path(&destination).expect("read conflicting identity");
            fs::set_permissions(&destination, permissions).expect("restore fixture permissions");

            let Error::Recovery {
                source,
                backup,
                unrecovered,
            } = error
            else {
                panic!("report incomplete recovery");
            };

            assert_eq!(unrecovered, [destination, other.clone()]);
            assert!(backup.is_dir());

            if absent {
                assert!(!backup.join("original").exists());
            } else {
                assert_eq!(
                    fs::read_to_string(backup.join("original").join(relatives[0]))
                        .expect("retain original backup"),
                    format!("old {}", relatives[0]),
                );
            }

            assert_eq!(
                fs::read_to_string(other).expect("retain another conflicting destination"),
                "another concurrent edit",
            );

            assert!(matches!(
                *source,
                Error::Filesystem { path, source }
                    if path == failure && source.to_string() == "injected commit failure"
            ));

            let recovered = fixture.directory.join(relatives[1]);

            if absent {
                assert!(!recovered.exists());
            } else {
                assert_eq!(
                    fs::read_to_string(recovered).expect("recover independent destination"),
                    format!("old {}", relatives[1]),
                );
            }

            match conflict {
                Conflict::Contents => assert_eq!(retained, "concurrent edit"),
                Conflict::Permissions => assert!(readonly),
                Conflict::Identity => assert_ne!(identity, committed[2].identity),
            }
        }
    }
}

#[test]
fn publication_holds_standard_lock_from_snapshot_through_completion() {
    let fixture = Fixture::new();
    let publication = fixture.publication();

    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(fixture.root.path().join("output.lock"))
        .expect("open publication lock independently");

    assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));

    publication
        .write(&fixture.files, || false)
        .expect("finish locked publication");

    lock.try_lock()
        .expect("release publication lock after completion");
}
