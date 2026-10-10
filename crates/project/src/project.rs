use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
};

use instar_shared::{Options, SourceIdentity, SourceSnapshot};
use serde_json::Value;

use crate::{
    assets::{AssetRequest, AssetSnapshot},
    capture::Capture,
    change::{ChangeBatch, ProjectChange},
    configuration::ProjectSettingsPatch,
    editing::{ApplyRequest, EditRequest},
    execution::{Execution, Host, Task},
    inputs::{InputVersion, ProjectSnapshot, initial},
    queries::QueryRequest,
    report::{
        ApplyReport, CheckReport, EditReport, FormatReport, LintReport, QueryReport, SettingsReport,
    },
    request::{CheckRequest, FormatRequest, LintRequest, SettingsRequest},
    watch::{WatchReport, WatchRequest},
};

/// Thread-safe owner of captured project state and isolated operation workers.
pub struct Project {
    pub(crate) host: Arc<Host>,
    execution: Execution,
}

impl Project {
    /// Creates a project with the supplied workspace roots.
    ///
    /// Workers start lazily when operations are submitted.
    #[must_use]
    pub fn new(roots: Vec<PathBuf>) -> Self {
        let host = Arc::new(Host {
            snapshot: RwLock::new(initial(roots)),
            next_source: AtomicU64::new(1),
            next_invocation: AtomicU64::new(1),
            worker_epoch: AtomicU64::new(0),
            module_contexts: Mutex::default(),
        });

        Self {
            execution: Execution::new(Arc::clone(&host)),
            host,
        }
    }

    /// Allocates an identity for a source to register with the project.
    pub fn source_identity(&self) -> SourceIdentity {
        self.host.source_identity()
    }

    /// Returns an immutable view of the current project state.
    pub fn snapshot(&self) -> ProjectSnapshot {
        self.host.snapshot()
    }

    /// Captures current source contents and their immutable project snapshot.
    ///
    /// # Errors
    /// Returns interruption, filesystem, encoding, or conflicting-observation errors.
    pub fn capture(
        &self,
        path: &Path,
        options: &Options,
    ) -> io::Result<(ProjectSnapshot, SourceSnapshot)> {
        let mut capture = Capture::live(Arc::clone(&self.host));
        let source = capture.source(path, options)?;

        Ok((capture.snapshot, source))
    }

    /// Reports whether all consulted inputs are fresh and match current state.
    pub fn current(&self, inputs: &[InputVersion]) -> bool {
        let snapshot = self.snapshot();

        snapshot.current(inputs)
    }

    /// Applies a change batch atomically and returns the resulting snapshot.
    ///
    /// # Errors
    /// Returns invalid-change, stale-revision, path-normalization, or revision-exhaustion errors.
    pub fn change(&self, batch: ChangeBatch) -> io::Result<ProjectSnapshot> {
        let discarded = batch
            .changes
            .iter()
            .any(|change| matches!(change, ProjectChange::WorkerDiscarded(_)));

        let mut state = self
            .host
            .snapshot
            .write()
            .unwrap_or_else(PoisonError::into_inner);

        let next = state.transition(batch)?;
        *state = next.clone();

        if discarded {
            self.host.worker_epoch.fetch_add(1, Ordering::Release);
        }

        Ok(next)
    }

    /// Schedules dependency-aware checking; awaiting the task reports operation failures.
    pub fn check(&self, request: CheckRequest, options: Options) -> Task<CheckReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.check(&request, None, options)
        })
    }

    /// Schedules formatting in the requested output mode.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn format(&self, request: FormatRequest, options: Options) -> Task<FormatReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.format(&request, options)
        })
    }

    /// Schedules linting and the requested fix processing.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn lint(&self, request: LintRequest, options: Options) -> Task<LintReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.lint(&request, None, options)
        })
    }

    /// Schedules a query against its captured snapshot.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn query(&self, request: QueryRequest, options: Options) -> Task<QueryReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.query(request, options)
        })
    }

    /// Schedules edit preparation and its required validation.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn edit(&self, request: EditRequest, options: Options) -> Task<EditReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.edit(request, options)
        })
    }

    /// Schedules explicitly authorized application of a validated edit.
    ///
    /// Awaiting the task reports operation failures; individual write outcomes are retained.
    pub fn apply(&self, request: ApplyRequest, options: Options) -> Task<ApplyReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.apply(request, options)
        })
    }

    /// Schedules effective settings capture for the requested operation.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn settings(&self, request: SettingsRequest, options: Options) -> Task<SettingsReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.settings(request, options)
        })
    }

    /// Schedules watched execution until interruption.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn watch(&self, request: WatchRequest, options: Options) -> Task<WatchReport> {
        self.execution.submit(options, move |runtime, options| {
            runtime.watch(request, options)
        })
    }

    /// Schedules digest-pinned acquisition into an explicitly authorized asset cache.
    ///
    /// Operation failures are returned when the task is awaited.
    pub fn acquire(&self, request: AssetRequest, options: Options) -> Task<AssetSnapshot> {
        self.execution.submit(options, move |runtime, options| {
            runtime.acquire(request, options)
        })
    }

    /// Returns the project configuration schema.
    #[must_use]
    pub fn configuration_schema() -> Value {
        serde_json::to_value(schemars::schema_for!(ProjectSettingsPatch)).unwrap_or(Value::Null)
    }

    /// Stops admission and requests cancellation of queued and running operations.
    pub fn stop(&self) {
        self.execution.stop();
    }

    /// Stops operations and asynchronously waits for worker cleanup.
    ///
    /// # Errors
    /// Returns cleanup-thread startup, join, or worker shutdown failures.
    pub async fn finish(&self) -> io::Result<()> {
        self.execution.finish().await
    }
}
