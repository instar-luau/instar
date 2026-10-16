use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use eyre::Result;

use instar_workspace::Workspace;

use notify::{
    Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _, event::ModifyKind,
};

use super::arguments::Options;

pub(super) enum Change {
    Sources(BTreeSet<String>),
    Reload,
}

pub(super) struct Watcher {
    subscription: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    directories: BTreeMap<PathBuf, bool>,
    observed: BTreeSet<PathBuf>,
    debounce: Duration,
    quiet: bool,
}

impl Watcher {
    pub(super) fn new(root: &Path, options: &Options, quiet: bool) -> Result<Self> {
        let mut directories = BTreeMap::from([(root.to_owned(), true)]);

        if options.configuration.is_empty() {
            for ancestor in root.ancestors().skip(1) {
                directories.insert(ancestor.to_owned(), false);
            }
        }

        for input in options.inputs.iter().chain(&options.configuration) {
            let path = root.join(input);

            if path.starts_with(root)
                && !path
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                continue;
            }

            if let Some(directory) = directory(&path)? {
                directories.insert(directory.canonicalize()?, true);
            }
        }

        let (sender, receiver) = mpsc::channel();
        let mut subscription = notify::recommended_watcher(sender)?;

        for (directory, recursive) in &directories {
            subscription.watch(
                directory,
                if *recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                },
            )?;
        }

        Ok(Self {
            subscription,
            receiver,
            directories,
            observed: BTreeSet::new(),
            debounce: Duration::from_millis(options.debounce),
            quiet,
        })
    }

    pub(super) fn begin(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        self.observed.clear();

        self.observe(paths)
    }

    pub(super) fn observe(&mut self, paths: impl IntoIterator<Item = PathBuf>) -> Result<()> {
        for path in paths {
            let discovery = path
                .file_name()
                .is_some_and(|name| name == "instar.toml" || name == "sourcemap.json");

            if !path
                .components()
                .any(|component| component == Component::ParentDir)
                && self.directories.iter().any(|(directory, recursive)| {
                    (*recursive && path.starts_with(directory))
                        || (discovery && path.parent() == Some(directory.as_path()))
                })
                || !self.observed.insert(path.clone())
            {
                continue;
            }

            let Some(ancestor) = directory(&path)? else {
                continue;
            };

            let directory = ancestor.canonicalize()?;
            let discovery = discovery && path.parent() == Some(ancestor);

            if self.directories.iter().any(|(watched, recursive)| {
                (*recursive && directory.starts_with(watched))
                    || (discovery && &directory == watched)
            }) {
                continue;
            }

            if self.directories.contains_key(&directory) {
                self.subscription.unwatch(&directory)?;
            }

            self.subscription.watch(
                &directory,
                if discovery {
                    RecursiveMode::NonRecursive
                } else {
                    RecursiveMode::Recursive
                },
            )?;

            self.directories.insert(directory, !discovery);
        }

        Ok(())
    }

    pub(super) fn changed(&self, workspace: &Workspace) -> Result<Change> {
        let mut dirty = BTreeSet::new();

        for event in self.wait()? {
            if event.need_rescan() || event.paths.is_empty() || !modified(&event) {
                return Ok(Change::Reload);
            }

            for path in event.paths {
                if path.is_dir() {
                    continue;
                }

                if !path
                    .extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
                {
                    return Ok(Change::Reload);
                }

                let module = workspace.module(&path)?;

                if workspace.requires_reload(&module)? {
                    return Ok(Change::Reload);
                }

                dirty.insert(module);
            }
        }

        Ok(Change::Sources(dirty))
    }

    pub(super) fn wait(&self) -> Result<Vec<Event>> {
        if !self.quiet {
            eprintln!("Watching for changes");
        }

        let mut events = Vec::new();

        loop {
            let event = self.receiver.recv()??;

            if self.relevant(&event) {
                events.push(event);
                break;
            }
        }

        let mut updated = Instant::now();

        loop {
            match self
                .receiver
                .recv_timeout(self.debounce.saturating_sub(updated.elapsed()))
            {
                Ok(event) => {
                    let event = event?;

                    if self.relevant(&event) {
                        events.push(event);
                        updated = Instant::now();
                    }
                }

                Err(RecvTimeoutError::Timeout) => return Ok(events),
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn relevant(&self, event: &Event) -> bool {
        if event.need_rescan() {
            return true;
        }

        if matches!(event.kind, EventKind::Access(_)) {
            return false;
        }

        event.paths.iter().any(|path| {
            self.directories.iter().any(|(directory, recursive)| {
                if *recursive {
                    path.starts_with(directory)
                } else {
                    path.parent() == Some(directory.as_path())
                        && path
                            .file_name()
                            .is_some_and(|name| name == "instar.toml" || name == "sourcemap.json")
                }
            }) && (!modified(event) || !path.is_dir())
        })
    }
}

fn directory(path: &Path) -> Result<Option<&Path>> {
    for ancestor in path.ancestors() {
        match ancestor.metadata() {
            Ok(metadata) if metadata.is_dir() => return Ok(Some(ancestor)),
            Ok(_) => {}

            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}

            Err(error) => return Err(error.into()),
        }
    }

    Ok(None)
}

fn modified(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    )
}
