use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, TryLockError},
    thread,
    time::{Duration, Instant},
};

use instar_shared::{
    Completion, Event, EventPayload, OperationFailure, Options, SourceIdentity, SourceSnapshot,
    SourceVersion,
};

use notify::{
    Error as NotificationError, ErrorKind as NotificationErrorKind, Event as NotificationEvent,
    EventKind, RecommendedWatcher, RecursiveMode, Result as NotificationResult, Watcher,
    recommended_watcher,
};

use crate::{
    change::{ChangeBatch, ProjectChange, SourceChange},
    execution::{Host, Runtime, failure},
    inputs::{InputIdentity, InputState, InputVersion, ProjectSnapshot, SourceOrigin, absolute},
    report::{CheckReport, FormatReport, LintReport},
    request::{CheckRequest, FixMode, FormatMode, FormatRequest, Input, Invocation, LintRequest},
};

const INTERRUPTION_INTERVAL: Duration = Duration::from_millis(10);

/// A command and polling interval for one watch iteration.
#[derive(Clone, Debug)]
pub struct WatchRequest {
    /// The command to run initially or after consulted inputs change.
    pub operation: WatchOperation,

    /// Interval between polling consulted filesystem inputs.
    pub poll_interval: Duration,
}

/// The command whose selected inputs and settings are watched.
#[derive(Clone, Debug)]
pub enum WatchOperation {
    /// Recheck selected sources and their dependencies.
    Check(CheckRequest),

    /// Reformat selected sources under the requested output mode.
    Format(FormatRequest),

    /// Relint selected sources under the requested fix policy.
    Lint(LintRequest),
}

impl WatchRequest {
    fn validate(&self) -> io::Result<()> {
        if self.poll_interval.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch polling interval must be positive",
            ));
        }

        if matches!(&self.operation, WatchOperation::Format(request) if matches!(request.mode, FormatMode::Source))
            || matches!(&self.operation, WatchOperation::Lint(request) if matches!(request.fixes, FixMode::Stdin))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "watch cannot emit formatted or fixed source",
            ));
        }

        Ok(())
    }
}

impl WatchOperation {
    fn invocation(&self) -> &Invocation {
        match self {
            Self::Check(request) => &request.invocation,
            Self::Format(request) => &request.invocation,
            Self::Lint(request) => &request.invocation,
        }
    }
}

/// Command results retained from a watch iteration.
#[derive(Debug)]
pub enum WatchCommandReport {
    /// Checking results against captured inputs.
    Check(CheckReport),

    /// Formatting results against captured inputs.
    Format(FormatReport),

    /// Linting and fix results against captured inputs.
    Lint(LintReport),
}

impl WatchCommandReport {
    pub(crate) fn snapshot(&self) -> &ProjectSnapshot {
        match self {
            Self::Check(report) => &report.snapshot,
            Self::Format(report) => &report.snapshot,
            Self::Lint(report) => &report.snapshot,
        }
    }

    /// Returns the consulted input versions used to check currency.
    #[must_use]
    pub fn inputs(&self) -> &[InputVersion] {
        match self {
            Self::Check(report) => &report.inputs,
            Self::Format(report) => &report.inputs,
            Self::Lint(report) => &report.inputs,
        }
    }

    pub(crate) fn completion(&self) -> &Completion {
        match self {
            Self::Check(report) => &report.completion,
            Self::Format(report) => &report.completion,
            Self::Lint(report) => &report.completion,
        }
    }
}

/// A command report with its captured snapshot and triggering input changes.
#[derive(Debug)]
pub struct WatchIteration {
    /// The exact snapshot retained by this iteration's command report.
    pub snapshot: ProjectSnapshot,

    /// Consulted input versions that changed since the previous iteration.
    pub changed: Vec<InputVersion>,

    /// Command results against the captured inputs.
    pub report: WatchCommandReport,
}

/// Collected watch iterations, events, and terminal outcomes.
#[derive(Debug)]
pub struct WatchReport {
    /// Snapshot captured before waiting or at the completed iteration's guard comparison.
    pub snapshot: ProjectSnapshot,

    /// Iterations that passed the project-side consulted-input guard comparison.
    pub iterations: Vec<WatchIteration>,

    /// Collected iterations withheld from current publication.
    pub historical: Vec<WatchIteration>,

    /// Events collected during this watch request.
    pub events: Vec<Event>,

    /// A terminal invocation failure retained with the collected outcomes.
    pub failure: Option<OperationFailure>,

    /// Combined coverage and interruption outcome for this watch request.
    pub completion: Completion,
}

impl WatchReport {
    fn retain(
        &mut self,
        result: io::Result<()>,
        events: &Mutex<Vec<Event>>,
        options: &Options,
    ) -> io::Result<()> {
        self.events
            .clone_from(&events.lock().unwrap_or_else(PoisonError::into_inner));

        if let Err(error) = result {
            let recorded = self.events.iter().any(|event| {
                matches!(
                    &event.payload,
                    EventPayload::Diagnostic(_)
                        | EventPayload::SourceOutcome(_)
                        | EventPayload::EditOutcome(_)
                )
            });

            if self.historical.is_empty()
                && self.iterations.is_empty()
                && !recorded
                && options.interrupted().is_none()
            {
                return Err(error);
            }

            let failure = failure(&error, options, events);
            self.completion = self.completion.clone().combine(failure.completion.clone());
            self.failure = Some(failure);
        }

        if let Some(completion) = options.interrupted() {
            self.completion = self.completion.clone().combine(completion);
        }

        Ok(())
    }
}

#[derive(Default)]
struct Pending {
    rescan: bool,
    failure: Option<NotificationError>,
}

struct Baseline {
    snapshot: ProjectSnapshot,
    inputs: Vec<InputVersion>,
}

#[derive(Default)]
pub(crate) struct Watch {
    native: Option<RecommendedWatcher>,
    started: bool,
    registrations: BTreeMap<PathBuf, bool>,
    pending: Arc<Mutex<Pending>>,
    operation: Option<String>,
    baseline: Option<Baseline>,
}

impl Watch {
    pub(crate) fn clear(&mut self) -> io::Result<()> {
        let mut failure = None;

        if let Some(native) = &mut self.native {
            for path in self.registrations.keys() {
                if let Err(error) = native.unwatch(path)
                    && !matches!(error.kind, NotificationErrorKind::WatchNotFound)
                    && failure.is_none()
                {
                    failure = Some(io::Error::other(error));
                }
            }
        }

        *self = Self::default();

        failure.map_or(Ok(()), Err)
    }

    fn register(&mut self, paths: BTreeMap<PathBuf, bool>, options: &Options) -> io::Result<()> {
        options.check()?;

        if !self.started {
            let pending = Arc::clone(&self.pending);

            match recommended_watcher(move |result: NotificationResult<NotificationEvent>| {
                let mut pending = pending.lock().unwrap_or_else(PoisonError::into_inner);

                match result {
                    Ok(event) if !matches!(event.kind, EventKind::Access(_)) => {
                        pending.rescan = true;
                    }

                    Ok(_) => {}

                    Err(error) => {
                        if pending.failure.is_none() {
                            pending.failure = Some(error);
                        }
                    }
                }
            }) {
                Ok(native) => self.native = Some(native),
                Err(error) if unsupported(&error) => {}
                Err(error) => return Err(io::Error::other(error)),
            }

            self.started = true;
        }

        if let Some(native) = &mut self.native {
            for (path, recursive) in &paths {
                options.check()?;

                if self.registrations.get(path) == Some(recursive) {
                    continue;
                }

                if self.registrations.contains_key(path) {
                    native.unwatch(path).map_err(io::Error::other)?;
                }

                let mode = if *recursive {
                    RecursiveMode::Recursive
                } else {
                    RecursiveMode::NonRecursive
                };

                native.watch(path, mode).map_err(io::Error::other)?;
            }

            for path in self
                .registrations
                .keys()
                .filter(|path| !paths.contains_key(*path))
            {
                options.check()?;

                match native.unwatch(path) {
                    Ok(()) => {}
                    Err(error) if matches!(error.kind, NotificationErrorKind::WatchNotFound) => {}
                    Err(error) => return Err(io::Error::other(error)),
                }
            }
        }

        self.registrations = paths;

        Ok(())
    }

    fn signalled(&self) -> io::Result<bool> {
        let mut pending = self.pending.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(error) = pending.failure.take() {
            return Err(io::Error::other(error));
        }

        let signalled = pending.rescan;
        pending.rescan = false;

        Ok(signalled)
    }

    fn changes(
        &mut self,
        host: &Arc<Host>,
        initial: &BTreeMap<PathBuf, bool>,
        poll_interval: Duration,
        options: &Options,
    ) -> io::Result<BTreeMap<InputIdentity, InputVersion>> {
        let mut changed = BTreeMap::new();

        let Some(baseline) = &self.baseline else {
            self.register(initial.clone(), options)?;

            return Ok(changed);
        };

        let mut paths = initial.clone();
        extend_registrations(&mut paths, &baseline.snapshot, &baseline.inputs, options)?;
        self.register(paths, options)?;

        let baseline = self
            .baseline
            .as_ref()
            .ok_or_else(|| io::Error::other("watch baseline disappeared"))?;

        let mut polled = Instant::now();

        loop {
            options.check()?;

            if self.signalled()?
                || polled.elapsed() >= poll_interval
                || !host.snapshot().current(&baseline.inputs)
            {
                let snapshot = refresh(host, &baseline.snapshot, &baseline.inputs, options)?;
                collect_changes(&baseline.inputs, &snapshot, &mut changed);
                polled = Instant::now();

                if !changed.is_empty() {
                    return Ok(changed);
                }
            }

            pause(options);
        }
    }
}

impl Runtime {
    pub(crate) fn watch(
        &mut self,
        request: WatchRequest,
        options: &Options,
    ) -> io::Result<WatchReport> {
        request.validate()?;

        let WatchRequest {
            operation,
            poll_interval,
        } = request;

        let shared = Arc::clone(&self.watch);
        self.workers.leave();

        let mut watch = loop {
            options.check()?;

            match shared.try_lock() {
                Ok(watch) => break watch,
                Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
                Err(TryLockError::WouldBlock) => pause(options),
            }
        };

        let key = format!("{operation:?}");

        if watch.operation.as_ref() != Some(&key) {
            watch.clear()?;
            watch.operation = Some(key);
        }

        let mut output = WatchReport {
            snapshot: self.host.snapshot(),
            iterations: Vec::new(),
            historical: Vec::new(),
            events: Vec::new(),
            failure: None,
            completion: Completion::Complete,
        };

        let result = (|| {
            let initial = registrations(operation.invocation(), &output.snapshot, &[], options)?;
            let changed = watch.changes(&self.host, &initial, poll_interval, options)?;

            self.watch_iteration(
                operation,
                initial,
                changed,
                &mut watch,
                &mut output,
                options,
            )
        })();

        self.workers.leave();
        output.retain(result, &self.events, options)?;

        Ok(output)
    }

    fn watch_iteration(
        &mut self,
        operation: WatchOperation,
        mut paths: BTreeMap<PathBuf, bool>,
        changed: BTreeMap<InputIdentity, InputVersion>,
        watch: &mut Watch,
        output: &mut WatchReport,
        options: &Options,
    ) -> io::Result<()> {
        let events = Arc::new(Mutex::new(Vec::<Event>::new()));
        let report = self.watch_command(operation, &events, options)?;
        let snapshot = report.snapshot().clone();
        let inputs = report.inputs().to_vec();
        output.completion = report.completion().clone();

        output.historical.push(WatchIteration {
            snapshot: snapshot.clone(),
            changed: changed.into_values().collect(),
            report,
        });

        extend_registrations(&mut paths, &snapshot, &inputs, options)?;
        watch.register(paths, options)?;
        watch.signalled()?;
        let current = refresh(&self.host, &snapshot, &inputs, options)?;
        let publish = current.current(&inputs);
        output.snapshot = current;
        watch.baseline = Some(Baseline { snapshot, inputs });

        if publish {
            let iteration = output
                .historical
                .pop()
                .ok_or_else(|| io::Error::other("watch iteration disappeared"))?;

            output.iterations.push(iteration);

            if let Some(observer) = &self.observer {
                let events = events
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone();

                for event in events {
                    observer(event);
                }
            }
        }

        Ok(())
    }

    fn watch_command(
        &mut self,
        operation: WatchOperation,
        events: &Arc<Mutex<Vec<Event>>>,
        options: &Options,
    ) -> io::Result<WatchCommandReport> {
        self.workers.enter(options)?;
        let captured = Arc::clone(events);
        let retained = Arc::clone(&self.events);
        let publication = self.observer.clone();
        let invocation = self.invocation;
        let mut iteration_options = options.clone();

        iteration_options.observer = Some(Arc::new(move |mut event| {
            event.invocation = invocation;
            event.operation = invocation;

            retained
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event.clone());

            if matches!(
                &event.payload,
                EventPayload::Progress(_) | EventPayload::Log(_) | EventPayload::InputPath(_)
            ) {
                if let Some(observer) = &publication {
                    observer(event);
                }
            } else {
                captured
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(event);
            }
        }));

        let report = match operation {
            WatchOperation::Check(request) => self
                .check(&request, None, &iteration_options)
                .map(WatchCommandReport::Check),

            WatchOperation::Format(request) => self
                .format(&request, &iteration_options)
                .map(WatchCommandReport::Format),

            WatchOperation::Lint(request) => self
                .lint(&request, None, &iteration_options)
                .map(WatchCommandReport::Lint),
        };

        self.workers.leave();

        report
    }
}

enum Observation {
    File {
        path: PathBuf,
        text: Option<Arc<str>>,
    },

    Directory {
        path: PathBuf,
        paths: Option<Vec<PathBuf>>,
    },
}

impl Observation {
    fn file(path: PathBuf, options: &Options) -> io::Result<Self> {
        options.check()?;

        let text = match fs::read(&path) {
            Ok(bytes) => Some(
                SourceSnapshot::from_bytes(
                    SourceVersion {
                        identity: SourceIdentity::new(0),
                        revision: 1,
                    },
                    &bytes,
                )?
                .text,
            ),

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }

            Err(error) => {
                return Err(io::Error::new(
                    error.kind(),
                    format!("{}: {error}", path.display()),
                ));
            }
        };

        Ok(Self::File { path, text })
    }

    fn directory(path: PathBuf, options: &Options) -> io::Result<Self> {
        options.check()?;

        let paths = match fs::read_dir(&path) {
            Ok(entries) => {
                let mut paths = Vec::new();

                for entry in entries {
                    options.check()?;
                    paths.push(entry?.path());
                }

                paths.sort();

                Some(paths)
            }

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                None
            }

            Err(error) => return Err(error),
        };

        Ok(Self::Directory { path, paths })
    }

    fn apply(
        self,
        host: &Host,
        snapshot: &mut ProjectSnapshot,
        changes: &mut Vec<ProjectChange>,
    ) -> io::Result<()> {
        match self {
            Self::File { path, text } => {
                let previous = snapshot.source_at(&path).cloned();

                let change = match (previous, text) {
                    (Some(source), Some(text)) if source.text != text => {
                        Some(SourceChange::Replace {
                            expected: source.version,
                            text,
                        })
                    }

                    (Some(source), None) => Some(SourceChange::Remove {
                        expected: source.version,
                    }),

                    (None, Some(text)) => Some(SourceChange::Open {
                        identity: host.source_identity(),
                        origin: SourceOrigin::Disk(path.clone()),
                        text,
                    }),

                    (Some(_), Some(_)) | (None, None) => None,
                };

                if let Some(change) = change {
                    append(snapshot, changes, ProjectChange::Source(change))?;
                }

                let state = snapshot
                    .source_at(&path)
                    .cloned()
                    .map_or(InputState::Missing, InputState::Source);

                observe(snapshot, changes, InputIdentity::Path(path), state)
            }

            Self::Directory { path, paths } => observe(
                snapshot,
                changes,
                InputIdentity::Directory(path),
                paths.map_or(InputState::Missing, InputState::Directory),
            ),
        }
    }
}

fn refresh(
    host: &Arc<Host>,
    captured: &ProjectSnapshot,
    inputs: &[InputVersion],
    options: &Options,
) -> io::Result<ProjectSnapshot> {
    loop {
        options.check()?;
        let before = host.snapshot();
        let observations = observations(&before, captured, inputs, options)?;
        options.check()?;
        let changes = changes(host, &before, observations, inputs)?;

        let mut current = host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        if current.identity != before.identity {
            continue;
        }

        if !changes.is_empty() {
            *current = current.transition(ChangeBatch { changes })?;
        }

        return Ok(current.clone());
    }
}

fn observations(
    current: &ProjectSnapshot,
    captured: &ProjectSnapshot,
    inputs: &[InputVersion],
    options: &Options,
) -> io::Result<Vec<Observation>> {
    let mut files = BTreeSet::new();
    let mut directories = BTreeSet::new();

    for input in inputs {
        match &input.identity {
            InputIdentity::Path(path) => {
                files.insert(path.clone());
            }

            InputIdentity::Directory(path) => {
                directories.insert(path.clone());
            }

            InputIdentity::Source(identity)
            | InputIdentity::Configuration(identity)
            | InputIdentity::Sourcemap(identity) => {
                if let Some(SourceOrigin::Disk(path)) = current
                    .origin(*identity)
                    .or_else(|| captured.origin(*identity))
                {
                    files.insert(path.clone());
                }
            }

            InputIdentity::Asset(_) | InputIdentity::Context(_) | InputIdentity::Sources => {}
        }
    }

    let mut observed = Vec::new();

    for path in files {
        options.check()?;

        if current.source_at(&path).is_some_and(|source| {
            matches!(
                current.origin(source.version.identity),
                Some(SourceOrigin::Document { .. })
            )
        }) {
            continue;
        }

        observed.push(Observation::file(path, options)?);
    }

    for path in directories {
        observed.push(Observation::directory(path, options)?);
    }

    Ok(observed)
}

fn changes(
    host: &Host,
    snapshot: &ProjectSnapshot,
    observations: Vec<Observation>,
    inputs: &[InputVersion],
) -> io::Result<Vec<ProjectChange>> {
    let mut staged = snapshot.clone();
    let mut changes = Vec::new();

    for observation in observations {
        observation.apply(host, &mut staged, &mut changes)?;
    }

    for input in inputs {
        if let InputIdentity::Configuration(identity) | InputIdentity::Sourcemap(identity) =
            &input.identity
        {
            let state = staged
                .source(*identity)
                .cloned()
                .map_or(InputState::Missing, InputState::Source);

            observe(&mut staged, &mut changes, input.identity.clone(), state)?;
        }
    }

    Ok(changes)
}

fn append(
    snapshot: &mut ProjectSnapshot,
    changes: &mut Vec<ProjectChange>,
    change: ProjectChange,
) -> io::Result<()> {
    *snapshot = snapshot.transition(ChangeBatch {
        changes: vec![change.clone()],
    })?;

    changes.push(change);

    Ok(())
}

fn observe(
    snapshot: &mut ProjectSnapshot,
    changes: &mut Vec<ProjectChange>,
    identity: InputIdentity,
    state: InputState,
) -> io::Result<()> {
    let expected = snapshot.input_versions.get(&identity).cloned();

    if expected.as_ref().is_some_and(|input| input.state == state) {
        return Ok(());
    }

    let revision = expected
        .as_ref()
        .map_or(Some(1), |input| input.revision.checked_add(1))
        .ok_or_else(|| io::Error::other("watch input revision overflow"))?;

    append(
        snapshot,
        changes,
        ProjectChange::Input {
            expected,
            current: InputVersion {
                identity,
                revision,
                state,
            },
        },
    )
}

fn collect_changes(
    inputs: &[InputVersion],
    snapshot: &ProjectSnapshot,
    changed: &mut BTreeMap<InputIdentity, InputVersion>,
) {
    for input in inputs {
        if let Some(current) = snapshot.input_versions.get(&input.identity)
            && current != input
        {
            changed.insert(current.identity.clone(), current.clone());
        }
    }
}

fn registrations(
    invocation: &Invocation,
    snapshot: &ProjectSnapshot,
    inputs: &[InputVersion],
    options: &Options,
) -> io::Result<BTreeMap<PathBuf, bool>> {
    let mut paths = BTreeMap::new();

    if invocation.inputs.is_empty() {
        if let Some(root) = &invocation.project_root {
            register_location(&mut paths, root, true, options)?;
        } else {
            for root in snapshot.roots() {
                register_location(&mut paths, root, true, options)?;
            }
        }
    }

    for input in &invocation.inputs {
        match input {
            Input::Path(path) => register_location(&mut paths, path, true, options)?,
            Input::Pattern { base, .. } => register_location(&mut paths, base, true, options)?,

            Input::Stdin { context, .. } => {
                if let Some(path) = context {
                    register_location(&mut paths, path, false, options)?;
                }
            }

            Input::Source(identity) => {
                if let Some(path) = snapshot.origin(*identity).and_then(SourceOrigin::path) {
                    register_location(&mut paths, path, false, options)?;
                }
            }

            Input::Module(module) => {
                if let Some(path) = snapshot.origin(module.source).and_then(SourceOrigin::path) {
                    register_location(&mut paths, path, false, options)?;
                }
            }
        }
    }

    if let Some(path) = &invocation.configuration {
        register_location(&mut paths, path, false, options)?;
    }

    extend_registrations(&mut paths, snapshot, inputs, options)?;

    Ok(paths)
}

fn extend_registrations(
    paths: &mut BTreeMap<PathBuf, bool>,
    snapshot: &ProjectSnapshot,
    inputs: &[InputVersion],
    options: &Options,
) -> io::Result<()> {
    for input in inputs {
        match &input.identity {
            InputIdentity::Path(path) | InputIdentity::Directory(path) => {
                register_location(paths, path, false, options)?;
            }

            InputIdentity::Source(identity)
            | InputIdentity::Configuration(identity)
            | InputIdentity::Sourcemap(identity) => {
                if let Some(path) = snapshot.origin(*identity).and_then(SourceOrigin::path) {
                    register_location(paths, path, false, options)?;
                }
            }

            InputIdentity::Asset(_) | InputIdentity::Context(_) | InputIdentity::Sources => {}
        }
    }

    Ok(())
}

fn register_location(
    paths: &mut BTreeMap<PathBuf, bool>,
    path: &Path,
    recursive: bool,
    options: &Options,
) -> io::Result<()> {
    let location = absolute(path)?;
    let mut directory = location.as_path();
    let mut recursive = recursive;

    loop {
        options.check()?;

        match fs::metadata(directory) {
            Ok(metadata) if metadata.is_dir() => {
                paths
                    .entry(directory.to_path_buf())
                    .and_modify(|existing| *existing |= recursive)
                    .or_insert(recursive);

                return Ok(());
            }

            Ok(_) => {
                recursive = false;
            }

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
                ) =>
            {
                recursive = false;
            }

            Err(error) => return Err(error),
        }

        directory = directory.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "watch input has no existing parent directory",
            )
        })?;
    }
}

fn unsupported(error: &NotificationError) -> bool {
    matches!(&error.kind, NotificationErrorKind::Io(error) if error.kind() == io::ErrorKind::Unsupported)
}

fn pause(options: &Options) {
    thread::sleep(
        options
            .remaining()
            .map_or(INTERRUPTION_INTERVAL, |remaining| {
                remaining.min(INTERRUPTION_INTERVAL)
            }),
    );
}
