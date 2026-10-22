use std::{
    collections::{BTreeMap, BTreeSet},
    io::ErrorKind,
    iter, mem,
    path::{Component, Path, PathBuf},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    time::{Duration, Instant},
};

use eyre::{Result, WrapErr as _, eyre};

use instar_core::workspace::{Generators, Workspace, configuration_files, is_configuration};

use notify::{ErrorKind as WatchErrorKind, Event, EventKind, RecursiveMode, event::ModifyKind};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer_opt};

use super::{events::Events, project};
use crate::{arguments::Options, interrupt, output};

pub(super) enum Change {
    Sources {
        paths: BTreeSet<String>,
        should_discover: bool,
        started: Instant,
    },

    Reload {
        started: Instant,
    },
}

pub(crate) struct Watcher {
    subscription: Debouncer<Events, RecommendedCache>,
    receiver: Receiver<DebounceEventResult>,
    directories: BTreeMap<PathBuf, bool>,
    roots: BTreeSet<PathBuf>,
    reload: BTreeMap<PathBuf, bool>,
    observed: BTreeMap<PathBuf, PathBuf>,
    pub(super) generators: Generators,
}

impl Watcher {
    pub(super) fn new(root: &Path, options: &Options) -> Result<Self> {
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
                    format!(
                        "Could not resolve watch directory `{}`.",
                        directory.display()
                    )
                })?;

                directories.insert(canonical, true);
            }
        }

        let (sender, receiver) = mpsc::channel();

        let mut subscription = new_debouncer_opt(
            Duration::from_millis(options.debounce).max(Duration::from_millis(1)),
            None,
            sender,
            RecommendedCache::new(),
            notify::Config::default(),
        )
        .wrap_err("Could not start the filesystem watcher.")?;

        for (directory, is_recursive) in &directories {
            subscription
                .watch(
                    directory,
                    if *is_recursive {
                        RecursiveMode::Recursive
                    } else {
                        RecursiveMode::NonRecursive
                    },
                )
                .wrap_err_with(|| format!("Could not watch `{}`.", directory.display()))?;
        }

        let roots = directories.keys().cloned().collect();

        let mut watcher = Self {
            subscription,
            receiver,
            directories,
            roots,
            reload: BTreeMap::new(),
            observed: BTreeMap::new(),
            generators: Generators::new(&interrupt::CANCELLATION, output::generator),
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
        let previous = mem::replace(
            &mut self.generators,
            Generators::new(&interrupt::CANCELLATION, output::generator),
        );

        project::shutdown(&previous)?;

        Ok(())
    }

    pub(super) fn observe(
        &mut self,
        paths: impl IntoIterator<Item = PathBuf>,
        should_reload: bool,
    ) -> Result<()> {
        for path in paths {
            if let Some(target) = self.observed.get(&path)
                && (!should_reload || self.reload.contains_key(target))
            {
                continue;
            }

            let Some(ancestor) = directory(&path)? else {
                continue;
            };

            let directory = ancestor.canonicalize().wrap_err_with(|| {
                format!(
                    "Could not resolve watch directory `{}`.",
                    ancestor.display()
                )
            })?;

            let target =
                directory.join(path.strip_prefix(ancestor).wrap_err(
                    "Internal error: the watched path is outside its ancestor directory.",
                )?);

            let is_recursive =
                target.parent() != Some(directory.as_path()) || self.roots.contains(&directory);

            self.observed.insert(path.clone(), target.clone());
            let canonical = canonical(&path)?;

            if should_reload {
                self.reload.insert(target.clone(), canonical.is_some());
            }

            let parent = target.parent().filter(|_| ancestor == path.as_path());

            for (directory, is_recursive) in parent
                .map(|parent| (parent.to_owned(), false))
                .into_iter()
                .chain(iter::once((directory, is_recursive)))
            {
                if self
                    .directories
                    .iter()
                    .any(|(watched, is_watched_recursively)| {
                        (*is_watched_recursively && directory.starts_with(watched))
                            || (!is_recursive && &directory == watched)
                    })
                {
                    continue;
                }

                if self.directories.contains_key(&directory) {
                    self.unwatch(&directory)?;
                }

                self.subscription
                    .watch(
                        &directory,
                        if is_recursive {
                            RecursiveMode::Recursive
                        } else {
                            RecursiveMode::NonRecursive
                        },
                    )
                    .wrap_err_with(|| format!("Could not watch `{}`.", directory.display()))?;

                self.directories.insert(directory, is_recursive);
            }

            if let Some(canonical) = canonical
                && canonical != target
            {
                self.observe([canonical], should_reload)?;
            }
        }

        Ok(())
    }

    pub(super) fn changed(&mut self, workspace: &Workspace) -> Result<Change> {
        let (events, has_generated_changes) = self.wait(false)?;
        let started = Instant::now();
        let mut paths = BTreeSet::new();
        let mut should_discover = false;

        for event in events {
            tracing::debug!(kind = ?event.kind, paths = ?event.paths, "Processing filesystem event");

            if event.need_rescan() || event.paths.is_empty() {
                tracing::debug!("Reloading after a filesystem rescan request");

                self.reset()?;

                return Ok(Change::Reload { started });
            }

            let is_structural = !is_modified(&event);

            for path in event.paths {
                if is_configuration(&path) && self.roots.iter().any(|root| path.starts_with(root)) {
                    self.reset()?;

                    return Ok(Change::Reload { started });
                }

                for (dependency, did_exist) in &self.reload {
                    if path.starts_with(dependency)
                        || (dependency.starts_with(&path)
                            && (*did_exist || canonical(dependency)?.is_some()))
                        || (dependency.parent() == path.parent()
                            && canonical(dependency)?.as_ref() == Some(&path))
                    {
                        tracing::debug!(path = %path.display(), "Reloading after a workspace dependency changed");

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

                should_discover |= is_structural;
                paths.insert(workspace.normalize(&path)?);
            }
        }

        if has_generated_changes {
            tracing::debug!("Reloading after generated sourcemaps changed");

            return Ok(Change::Reload { started });
        }

        Ok(Change::Sources {
            paths,
            should_discover,
            started,
        })
    }

    pub(super) fn wait(&mut self, is_recovering: bool) -> Result<(Vec<Event>, bool)> {
        let mut events = Vec::new();
        let mut has_generated_changes = false;

        loop {
            interrupt::check()?;

            if !is_recovering {
                has_generated_changes |= self.generators.advance()?;
            }

            let timeout = if has_generated_changes {
                Duration::ZERO
            } else {
                interrupt::INTERVAL
            };

            let batch = match self.receiver.recv_timeout(timeout) {
                Ok(events) => events.map_err(|errors| {
                    eyre!(
                        "Could not observe filesystem changes. {}",
                        errors
                            .iter()
                            .map(ToString::to_string)
                            .collect::<Vec<_>>()
                            .join("\n")
                    )
                })?,

                Err(RecvTimeoutError::Timeout) if has_generated_changes => break,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(error) => return Err(error).wrap_err("Could not receive filesystem changes."),
            };

            for event in batch {
                if self.is_relevant(&event.event, is_recovering)? {
                    events.push(event.event);
                }
            }

            if !events.is_empty() {
                break;
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

        Ok((events, has_generated_changes))
    }

    fn unwatch(&mut self, directory: &Path) -> Result<()> {
        match self.subscription.unwatch(directory) {
            Ok(()) => {}
            Err(error) if matches!(error.kind, WatchErrorKind::WatchNotFound) => {}

            Err(error) => {
                return Err(error).wrap_err_with(|| {
                    format!("Could not stop watching `{}`.", directory.display())
                });
            }
        }

        self.directories.remove(directory);

        Ok(())
    }

    fn is_relevant(&self, event: &Event, is_recovering: bool) -> Result<bool> {
        if event.need_rescan() || event.paths.is_empty() {
            return Ok(true);
        }

        if matches!(event.kind, EventKind::Access(_)) {
            return Ok(false);
        }

        for path in &event.paths {
            let is_directory = path.is_dir();

            if !is_recovering && is_modified(event) && is_directory {
                continue;
            }

            let is_source = path
                .extension()
                .is_some_and(|extension| extension == "lua" || extension == "luau")
                && !is_directory;

            if ((is_recovering || is_source || is_directory || is_configuration(path))
                && self.roots.iter().any(|root| path.starts_with(root)))
                || self.observed.values().any(|observed| {
                    observed.starts_with(path)
                        || ((is_recovering || is_source) && path.starts_with(observed))
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
                    .wrap_err_with(|| format!("Could not inspect `{}`.", ancestor.display()));
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

        Err(error) => Err(error)
            .wrap_err_with(|| format!("Could not resolve watched path `{}`.", path.display())),
    }
}

fn is_modified(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Metadata(_))
    )
}
