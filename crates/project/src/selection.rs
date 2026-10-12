use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Display,
    fs, io,
    path::{Path, PathBuf},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use ignore::{WalkBuilder, gitignore::GitignoreBuilder};
use instar_shared::{Completion, Event, EventPayload, Options, SourceIdentity};
use serde::Serialize;

use crate::{
    capture::{Capture, Listing},
    change::{ChangeBatch, ProjectChange, SourceChange},
    configuration::{EffectiveSettings, OperationOverrides},
    execution::Runtime,
    inputs::{InputIdentity, InputState, SourceOrigin, absolute},
    request::{Input, Invocation},
};

pub(crate) struct Matcher {
    compiled: GlobSet,
    empty: bool,
}

impl Matcher {
    pub(crate) fn new(patterns: Option<&[String]>) -> io::Result<Self> {
        let mut builder = GlobSetBuilder::new();

        for pattern in patterns.into_iter().flatten() {
            let pattern = pattern.replace('\\', "/");

            if pattern.is_empty()
                || pattern.starts_with('!')
                || pattern.contains('\0')
                || pattern
                    .split('/')
                    .any(|part| part.contains("**") && part != "**")
            {
                return Err(invalid("invalid selection pattern"));
            }

            let directory = pattern.ends_with('/');
            let pattern = pattern.trim_end_matches('/');
            let anchored = pattern.contains('/');

            let pattern = if anchored {
                pattern.to_owned()
            } else {
                format!("**/{pattern}")
            };

            if !directory {
                builder.add(
                    GlobBuilder::new(&pattern)
                        .literal_separator(true)
                        .backslash_escape(false)
                        .build()
                        .map_err(|error| invalid(format!("invalid glob: {error}")))?,
                );
            }

            builder.add(
                GlobBuilder::new(&format!("{pattern}/**"))
                    .literal_separator(true)
                    .backslash_escape(false)
                    .build()
                    .map_err(|error| invalid(format!("invalid glob: {error}")))?,
            );
        }

        Ok(Self {
            compiled: builder
                .build()
                .map_err(|error| invalid(format!("invalid glob: {error}")))?,
            empty: patterns.is_none_or(<[String]>::is_empty),
        })
    }

    pub(crate) fn matches(&self, path: &Path) -> bool {
        self.compiled
            .is_match(path.to_string_lossy().replace('\\', "/"))
    }

    pub(crate) fn includes(&self, path: &Path) -> bool {
        self.empty || self.matches(path)
    }
}

/// Selection evidence for one discovered or explicitly supplied input.
#[derive(Clone, Debug, Serialize)]
pub struct SelectionDecision {
    /// Display label of the candidate input.
    pub input: String,

    /// Captured source identity, when the candidate was loaded.
    pub source: Option<SourceIdentity>,

    /// Whether the candidate passed the effective selection policy.
    pub selected: bool,

    /// Explanation of inclusion or exclusion.
    pub reason: String,

    /// Ignore-file path responsible for the decision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<PathBuf>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SelectionPurpose {
    Command,
    Editor,
    Settings,
}

pub(crate) struct SelectedSource {
    pub(crate) identity: SourceIdentity,
    pub(crate) effective: EffectiveSettings,
}

pub(crate) struct SelectionResult {
    pub(crate) targets: Vec<SelectedSource>,
    pub(crate) candidates: usize,
    pub(crate) decisions: Vec<SelectionDecision>,
    pub(crate) completion: Completion,
}

struct InputCandidates {
    paths: BTreeMap<PathBuf, Option<PathBuf>>,
    sources: BTreeSet<SourceIdentity>,
}

impl Runtime {
    pub(crate) fn select(
        &mut self,
        invocation: &Invocation,
        overrides: &OperationOverrides,
        capture: &mut Capture,
        purpose: SelectionPurpose,
        options: &Options,
        mut ready: impl FnMut(&mut Self, &SelectedSource, &mut Capture, &Options) -> io::Result<()>,
    ) -> io::Result<SelectionResult> {
        overrides.selection().validate()?;
        let candidates = self.discover_inputs(invocation, capture, purpose, options)?;
        let mut ignored = Vec::new();
        let mut decisions = Vec::new();
        let mut identities = BTreeSet::new();
        let mut sources = candidates.sources.into_iter();
        let mut paths = candidates.paths.into_iter();
        let mut targets = Vec::new();

        loop {
            let identity = if let Some(identity) = sources.next() {
                identity
            } else if let Some((path, root)) = paths.next() {
                let Some(identity) =
                    capture_candidate(&path, root.as_deref(), &mut ignored, capture, options)?
                else {
                    continue;
                };

                identity
            } else {
                break;
            };

            if !identities.insert(identity) {
                continue;
            }

            options.check()?;

            let source = capture
                .snapshot
                .source(identity)
                .cloned()
                .ok_or_else(|| invalid("selected source disappeared"))?;

            capture.consult_source(&source);

            if let Some(path) = capture
                .snapshot
                .origin(identity)
                .and_then(SourceOrigin::path)
            {
                options.report(Event {
                    invocation: 0,
                    operation: 0,
                    payload: EventPayload::InputPath(path.to_path_buf()),
                });
            }

            let effective = self.effective(identity, invocation, overrides, capture, options)?;

            let path = capture
                .snapshot
                .origin(identity)
                .and_then(SourceOrigin::path);

            let policy = effective
                .settings
                .selection
                .operation(overrides.operation());

            let include = self.cache.matcher(policy.include.as_deref())?;
            let exclude = self.cache.matcher(policy.exclude.as_deref())?;
            let included = path.is_none_or(|path| include.includes(path));

            let excluded =
                path.is_some_and(|path| exclude.matches(path)) && !policy.override_exclusions;

            let selected = included && !excluded;

            decisions.push(SelectionDecision {
                input: path.map_or_else(
                    || format!("source:{}", identity.get()),
                    |path| path.display().to_string(),
                ),
                source: Some(identity),
                selected,
                reason: if excluded {
                    "configured exclusion"
                } else if !included {
                    "not matched by includes"
                } else {
                    "selected"
                }
                .to_owned(),
                origin: None,
            });

            if selected || purpose == SelectionPurpose::Settings {
                let target = SelectedSource {
                    identity,
                    effective,
                };

                ready(self, &target, capture, options)?;
                targets.push(target);
            }
        }

        targets.sort_by_key(|target| {
            capture
                .snapshot
                .origin(target.identity)
                .and_then(SourceOrigin::path)
                .map(Path::to_path_buf)
        });

        let candidate_count = identities.len() + ignored.len();
        ignored.extend(decisions);

        if candidate_count == 0 && purpose == SelectionPurpose::Command {
            return Err(invalid("empty discovery: no source candidates"));
        }

        Ok(SelectionResult {
            targets,
            candidates: candidate_count,
            decisions: ignored,
            completion: Completion::Complete,
        })
    }

    fn discover_inputs(
        &mut self,
        invocation: &Invocation,
        capture: &mut Capture,
        purpose: SelectionPurpose,
        options: &Options,
    ) -> io::Result<InputCandidates> {
        let mut candidates = BTreeMap::<PathBuf, Option<PathBuf>>::new();
        let mut identities = BTreeSet::new();

        let inputs = if invocation.inputs.is_empty() {
            if let Some(root) = &invocation.project_root {
                vec![Input::Path(root.clone())]
            } else {
                capture.snapshot.roots().cloned().map(Input::Path).collect()
            }
        } else {
            invocation.inputs.clone()
        };

        if inputs.is_empty() && purpose == SelectionPurpose::Command {
            return Err(invalid("empty discovery: no project roots or inputs"));
        }

        for input in inputs {
            options.check()?;

            match input {
                Input::Path(path) => {
                    let path = absolute(&path)
                        .map_err(|error| io::Error::new(error.kind(), format!("input {error}")))?;

                    if capture.snapshot.source_at(&path).is_some_and(|source| {
                        matches!(
                            capture.snapshot.origin(source.version.identity),
                            Some(SourceOrigin::Document { .. })
                        )
                    }) {
                        candidates.insert(path, None);
                    } else if fs::metadata(&path)?.is_dir() {
                        let paths = discover(&path, capture, options)?;
                        collect_candidates(&path, paths, &mut candidates);
                    } else {
                        candidates.insert(path, None);
                    }
                }

                Input::Pattern { base, pattern } => {
                    let base = absolute(&base)?;
                    let matcher = self.cache.matcher(Some(&[anchor(&base, &pattern)?]))?;

                    let paths = discover(&base, capture, options)?
                        .into_iter()
                        .filter(|path| matcher.matches(path))
                        .collect::<Vec<_>>();

                    if paths.is_empty() {
                        return Err(invalid(format!("unmatched input pattern {pattern:?}")));
                    }

                    collect_candidates(&base, paths, &mut candidates);
                }

                Input::Stdin { text, context } => {
                    if let Some(context) = &context {
                        absolute(context)?;
                    }

                    let identity = self.host.source_identity();

                    capture.observe(ChangeBatch {
                        changes: vec![ProjectChange::Source(SourceChange::Open {
                            identity,
                            origin: SourceOrigin::Stdin { context },
                            text,
                        })],
                    })?;

                    identities.insert(identity);
                }

                Input::Source(identity) => {
                    if capture.snapshot.source(identity).is_none() {
                        return Err(invalid("selected source identity has no contents"));
                    }

                    identities.insert(identity);
                }

                Input::Module(module) => {
                    if capture.snapshot.source(module.source).is_none() {
                        return Err(invalid("selected module source has no contents"));
                    }

                    identities.insert(module.source);
                }
            }
        }

        Ok(InputCandidates {
            paths: candidates,
            sources: identities,
        })
    }
}

pub(crate) fn anchor(base: &Path, pattern: &str) -> io::Result<String> {
    Matcher::new(Some(&[pattern.to_owned()]))?;
    let pattern = pattern.replace('\\', "/");

    let pattern = if pattern.trim_end_matches('/').contains('/') {
        pattern.trim_start_matches('/').to_owned()
    } else {
        format!("**/{pattern}")
    };

    Ok(format!(
        "{}/{}",
        base.to_string_lossy().replace('\\', "/"),
        pattern
    ))
}

fn capture_candidate(
    path: &Path,
    root: Option<&Path>,
    decisions: &mut Vec<SelectionDecision>,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<Option<SourceIdentity>> {
    options.check()?;

    if path.file_name().is_some_and(|name| name == "instar.luau") {
        return Err(invalid("instar.luau is not an importable module"));
    }

    if !source_path(path) {
        return Err(invalid(format!(
            "not a Lua or Luau source: {}",
            path.display()
        )));
    }

    let ignored = match root {
        Some(root) => ignored(path, root, capture, options)?,
        None => None,
    };

    if let Some((origin, reason)) = ignored {
        decisions.push(SelectionDecision {
            input: path.display().to_string(),
            source: None,
            selected: false,
            reason,
            origin,
        });

        return Ok(None);
    }

    let source = capture.source(path, options)?;

    Ok(Some(source.version.identity))
}

fn collect_candidates(
    root: &Path,
    paths: Vec<PathBuf>,
    candidates: &mut BTreeMap<PathBuf, Option<PathBuf>>,
) {
    for path in paths {
        candidates
            .entry(path)
            .and_modify(|existing| {
                if existing
                    .as_ref()
                    .is_some_and(|existing| root.starts_with(existing))
                {
                    *existing = Some(root.to_path_buf());
                }
            })
            .or_insert_with(|| Some(root.to_path_buf()));
    }
}

fn discover(root: &Path, capture: &mut Capture, options: &Options) -> io::Result<Vec<PathBuf>> {
    let mut paths = BTreeSet::new();

    if capture.frozen() {
        discover_frozen(root, capture, &mut paths, options)?;
    } else {
        let mut directories = BTreeSet::new();

        for entry in WalkBuilder::new(root)
            .hidden(false)
            .ignore(false)
            .git_ignore(false)
            .git_global(false)
            .git_exclude(false)
            .parents(false)
            .build()
        {
            options.check()?;
            let entry = entry.map_err(io::Error::other)?;

            if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                directories.insert(entry.path().to_path_buf());
            }

            if entry.file_type().is_some_and(|kind| kind.is_file()) && source_path(entry.path()) {
                paths.insert(entry.into_path());
            }
        }

        for directory in directories {
            capture.listing(&directory, options)?;
        }
    }

    let registered = capture
        .snapshot
        .sources()
        .filter_map(|source| {
            let origin = capture.snapshot.origin(source.version.identity)?;
            let path = origin.path()?;

            if !path.starts_with(root)
                || !source_path(path)
                || matches!(origin, SourceOrigin::Stdin { .. })
                || !capture.frozen() && matches!(origin, SourceOrigin::Disk(_))
                || capture
                    .snapshot
                    .input_versions
                    .get(&InputIdentity::Path(path.to_path_buf()))
                    .is_some_and(|input| input.state == InputState::Missing)
            {
                return None;
            }

            let refreshing = matches!(origin, SourceOrigin::Disk(_))
                && [
                    InputIdentity::Path(path.to_path_buf()),
                    InputIdentity::Source(source.version.identity),
                ]
                .iter()
                .any(|identity| {
                    capture
                        .snapshot
                        .input_versions
                        .get(identity)
                        .is_some_and(|input| input.state == InputState::Stale)
                });

            Some((path.to_path_buf(), refreshing))
        })
        .collect::<Vec<_>>();

    for (path, refreshing) in registered {
        if refreshing && capture.read(&path, options)?.is_none() {
            continue;
        }

        paths.insert(path);
    }

    Ok(paths.into_iter().collect())
}

fn discover_frozen(
    root: &Path,
    capture: &mut Capture,
    paths: &mut BTreeSet<PathBuf>,
    options: &Options,
) -> io::Result<()> {
    let mut pending = vec![root.to_path_buf()];
    let mut visited = BTreeSet::new();

    while let Some(directory) = pending.pop() {
        options.check()?;

        if !visited.insert(directory.clone()) {
            continue;
        }

        let Listing::Directory(entries) = capture.listing(&directory, options)? else {
            continue;
        };

        for path in entries {
            options.check()?;

            if capture
                .snapshot
                .input_versions
                .get(&InputIdentity::Path(path.clone()))
                .is_some_and(|input| {
                    matches!(input.state, InputState::Source(_) | InputState::Missing)
                })
            {
                if source_path(&path) && capture.read(&path, options)?.is_some() {
                    paths.insert(path);
                }

                continue;
            }

            if !capture
                .snapshot
                .input_versions
                .contains_key(&InputIdentity::Directory(path.clone()))
            {
                match fs::symlink_metadata(&path) {
                    Ok(metadata) if metadata.file_type().is_symlink() => continue,
                    Ok(_) => {}

                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                        ) =>
                    {
                        continue;
                    }

                    Err(error) => return Err(error),
                }
            }

            if matches!(capture.listing(&path, options)?, Listing::Directory(_)) {
                pending.push(path);
            } else if source_path(&path) {
                paths.insert(path);
            }
        }
    }

    Ok(())
}

fn ignored(
    path: &Path,
    root: &Path,
    capture: &mut Capture,
    options: &Options,
) -> io::Result<Option<(Option<PathBuf>, String)>> {
    let parent = path
        .parent()
        .ok_or_else(|| invalid("source has no parent"))?;

    let mut ancestors = parent.ancestors().collect::<Vec<_>>();
    ancestors.reverse();
    let mut decision = None;

    for ancestor in ancestors {
        for filename in [".gitignore", ".ignore", ".git/info/exclude"] {
            let file = ancestor.join(filename);

            if let Some(source) = capture.read(&file, options)? {
                let mut builder = GitignoreBuilder::new(ancestor);

                for line in source.text.lines() {
                    builder
                        .add_line(Some(file.clone()), line)
                        .map_err(io::Error::other)?;
                }

                let ignore_policy = builder.build().map_err(io::Error::other)?;
                let matched = ignore_policy.matched_path_or_any_parents(path, false);

                if matched.is_ignore() {
                    decision = Some((
                        Some(file.clone()),
                        format!(
                            "filesystem ignore: {}",
                            matched.inner().map_or("", |glob| glob.original())
                        ),
                    ));
                } else if matched.is_whitelist() {
                    decision = None;
                }
            }
        }
    }

    if decision.is_none()
        && path
            .strip_prefix(root)
            .map_err(invalid)?
            .components()
            .any(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|name| name.starts_with('.') && name != "." && name != "..")
            })
    {
        decision = Some((None, "hidden filesystem input".to_owned()));
    }

    Ok(decision)
}

fn source_path(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name != "instar.luau")
        && matches!(
            path.extension().and_then(|extension| extension.to_str()),
            Some("lua" | "luau")
        )
}

fn invalid(message: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.to_string())
}
