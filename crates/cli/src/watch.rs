use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    iter, mem,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr as _};

use instar_core::workspace::{Generators, Workspace, configuration_files};

use notify::{
    ErrorKind as WatchErrorKind, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher as _,
    event::ModifyKind,
};

use super::{arguments::Options, interrupt};

pub(super) enum Change {
    Sources {
        paths: BTreeSet<String>,
        discovery: bool,
        started: Instant,
    },

    Reload {
        started: Instant,
    },
}

pub(super) struct Watcher {
    subscription: RecommendedWatcher,
    receiver: Receiver<notify::Result<Event>>,
    directories: BTreeMap<PathBuf, bool>,
    roots: BTreeSet<PathBuf>,
    reload: BTreeMap<PathBuf, bool>,
    observed: BTreeMap<PathBuf, PathBuf>,
    pub(super) generators: Generators,
    debounce: Duration,
    quiet: bool,
}

impl Watcher {
    pub(super) fn new(root: &Path, options: &Options, quiet: bool) -> Result<Self> {
        let mut directories = BTreeMap::from([(root.to_owned(), true)]);

        for input in &options.inputs {
            let path = root.join(input);

            if path.starts_with(root)
                && !path
                    .components()
                    .any(|component| component == Component::ParentDir)
            {
                continue;
            }

            if let Some(directory) = directory(&path)? {
                let canonical = directory.canonicalize().wrap_err_with(|| {
                    format!("cannot resolve watch directory `{}`", directory.display())
                })?;

                directories.insert(canonical, true);
            }
        }

        let (sender, receiver) = mpsc::channel();

        let mut subscription =
            notify::recommended_watcher(sender).wrap_err("cannot start filesystem watcher")?;

        for (directory, recursive) in &directories {
            subscription
                .watch(
                    directory,
                    if *recursive {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )
                .wrap_err_with(|| format!("cannot watch `{}`", directory.display()))?;
        }

        let roots = directories.keys().cloned().collect();

        let mut watcher = Self {
            subscription,
            receiver,
            directories,
            roots,
            reload: BTreeMap::new(),
            observed: BTreeMap::new(),
            generators: Generators::new(interrupt::requested),
            debounce: Duration::from_millis(options.debounce),
            quiet,
        };

        watcher.observe(configuration_files(root, &options.configuration), true)?;

        Ok(watcher)
    }

    pub(super) fn begin(&mut self, workspace: &Workspace) -> Result<()> {
        self.observed.clear();
        self.reload.clear();
        self.observe(workspace.reload_dependencies(), true)?;
        self.observe(workspace.dependencies(), false)?;

        Ok(())
    }

    pub(super) fn reset(&mut self) -> Result<()> {
        let previous = mem::replace(&mut self.generators, Generators::new(interrupt::requested));
        previous.shutdown()?;

        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        reload: bool,
    ) -> Result<()> {
        for path in paths {
            if let Some(target) = self.observed.get(&path)
                && (!reload || self.reload.contains_key(target))
            {
                continue;
            }

            let Some(ancestor) = directory(&path)? else {
                continue;
            };

            let directory = ancestor.canonicalize().wrap_err_with(|| {
                format!("cannot resolve watch directory `{}`", ancestor.display())
            })?;

            let target = directory.join(
                path.strip_prefix(ancestor)
                    .wrap_err("internal error: watched path is outside its ancestor")?,
            );

            let recursive =
                target.parent() != Some(directory.as_path()) || self.roots.contains(&directory);

            self.observed.insert(path.clone(), target.clone());
            let canonical = canonical(&path)?;

            if reload {
                self.reload.insert(target.clone(), canonical.is_some());
            }

            let parent = target.parent().filter(|_| ancestor == path.as_path());

            for (directory, recursive) in parent
                .map(|parent| (parent.to_owned(), false))
                .into_iter()
                .chain(iter::once((directory, recursive)))
            {
                if self.directories.iter().any(|(watched, subtree)| {
                    (*subtree && directory.starts_with(watched))
                        || (!recursive && &directory == watched)
                }) {
                    continue;
                }

                if self.directories.contains_key(&directory) {
                    self.unwatch(&directory)?;
                }

                self.subscription
                    .watch(
                        &directory,
                        if recursive {
                            RecursiveMode::Recursive
                        } else {
                            RecursiveMode::NonRecursive
                        },
                    )
                    .wrap_err_with(|| format!("cannot watch `{}`", directory.display()))?;

                self.directories.insert(directory, recursive);
            }

            if let Some(canonical) = canonical
                && canonical != target
            {
                self.observe([canonical], reload)?;
            }
        }

        Ok(())
    }

    pub(super) fn changed(&mut self, workspace: &Workspace) -> Result<Change> {
        let (events, generated) = self.wait(false)?;
        let started = Instant::now();
        let mut paths = BTreeSet::new();
        let mut discovery = false;

        for event in events {
            tracing::debug!(kind = ?event.kind, paths = ?event.paths, "processing filesystem event");

            if event.need_rescan() || event.paths.is_empty() {
                tracing::debug!("reloading after filesystem rescan request");

                self.reset()?;

                return Ok(Change::Reload { started });
            }

            let structural = !modified(&event);

            for path in event.paths {
                for (dependency, existed) in &self.reload {
                    if path.starts_with(dependency)
                        || (dependency.starts_with(&path)
                            && (*existed || canonical(dependency)?.is_some()))
                        || (dependency.parent() == path.parent()
                            && canonical(dependency)?.as_ref() == Some(&path))
                    {
                        tracing::debug!(path = %path.display(), "reloading for workspace dependency");

                        self.reset()?;

                        return Ok(Change::Reload { started });
                    }
                }

                if !self.observed.values().any(|observed| observed == &path) {
                    for (original, observed) in &self.observed {
                        if observed.parent() == path.parent()
                            && canonical(observed)?.as_ref() == Some(&path)
                        {
                            paths.insert(workspace.normalize(original)?);
                        }
                    }
                }

                discovery |= structural;
                paths.insert(workspace.normalize(&path)?);
            }
        }

        if generated {
            tracing::debug!("reloading for generated sourcemaps");

            return Ok(Change::Reload { started });
        }

        Ok(Change::Sources {
            paths,
            discovery,
            started,
        })
    }

    pub(super) fn ready(&self) {
        if self.quiet {
            return;
        }

        eprintln!(
            "{} {}",
            console::style("watching").for_stderr().cyan().bold(),
            console::style("for changes").for_stderr().dim()
        );
    }

    pub(super) fn wait(&mut self, recovery: bool) -> Result<(Vec<Event>, bool)> {
        let mut events = Vec::new();
        let mut updated = Instant::now();
        let mut generated = false;

        loop {
            interrupt::check()?;

            if !recovery {
                generated |= self.generators.advance()?;
            }

            let remaining = self.debounce.saturating_sub(updated.elapsed());

            let timeout = if generated && events.is_empty() {
                Duration::ZERO
            } else if events.is_empty() {
                interrupt::INTERVAL
            } else {
                remaining.min(interrupt::INTERVAL)
            };

            let event = match self.receiver.recv_timeout(timeout) {
                Ok(event) => event.wrap_err("cannot observe filesystem changes")?,

                Err(RecvTimeoutError::Timeout) => {
                    if (generated && events.is_empty())
                        || (!events.is_empty() && updated.elapsed() >= self.debounce)
                    {
                        break;
                    }

                    continue;
                }

                Err(error) => return Err(error).wrap_err("cannot receive filesystem changes"),
            };

            if self.relevant(&event, recovery)? {
                events.push(event);
                updated = Instant::now();
            }
        }

        let removed: Vec<_> = self
            .directories
            .keys()
            .filter(|directory| {
                events.iter().any(|event| {
                    matches!(
                        event.kind,
                        EventKind::Remove(_) | EventKind::Modify(ModifyKind::Name(_))
                    ) && event.paths.iter().any(|path| directory.starts_with(path))
                })
            })
            .cloned()
            .collect();

        for directory in removed {
            self.unwatch(&directory)?;
        }

        Ok((events, generated))
    }

    fn unwatch(&mut self, directory: &Path) -> Result<()> {
        match self.subscription.unwatch(directory) {
            Ok(()) => {}
            Err(error) if matches!(error.kind, WatchErrorKind::WatchNotFound) => {}

            Err(error) => {
                return Err(error)
                    .wrap_err_with(|| format!("cannot stop watching `{}`", directory.display()));
            }
        }

        self.directories.remove(directory);

        Ok(())
    }

    fn relevant(&self, event: &Event, recovery: bool) -> Result<bool> {
        if event.need_rescan() || event.paths.is_empty() {
            return Ok(true);
        }

        if matches!(event.kind, EventKind::Access(_)) {
            return Ok(false);
        }

        for path in &event.paths {
            let directory = path.is_dir();

            if !recovery && modified(event) && directory {
                continue;
            }

            let source = path
                .extension()
                .is_some_and(|extension| extension == "lua" || extension == "luau")
                && !directory;

            if ((recovery || source || directory)
                && self.roots.iter().any(|root| path.starts_with(root)))
                || self.observed.values().any(|observed| {
                    observed.starts_with(path)
                        || ((recovery || source) && path.starts_with(observed))
                })
                || self
                    .reload
                    .keys()
                    .any(|dependency| path.starts_with(dependency))
            {
                return Ok(true);
            }

            for observed in self.observed.values() {
                if observed.parent() == path.parent() && canonical(observed)?.as_ref() == Some(path)
                {
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

fn directory(path: &Path) -> Result<Option<&Path>> {
    for ancestor in path.ancestors() {
        match ancestor.metadata() {
            Ok(metadata) if metadata.is_dir() => return Ok(Some(ancestor)),
            Ok(_) => {}

            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}

            Err(error) => {
                return Err(error)
                    .wrap_err_with(|| format!("cannot inspect `{}`", ancestor.display()));
            }
        }
    }

    Ok(None)
}

fn canonical(path: &Path) -> Result<Option<PathBuf>> {
    match path.canonicalize() {
        Ok(path) => Ok(Some(path)),

        Err(error) if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
            Ok(None)
        }

        Err(error) => {
            Err(error).wrap_err_with(|| format!("cannot resolve watched path `{}`", path.display()))
        }
    }
}

fn modified(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    )
}
