use std::{
    collections::HashMap,
    io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use instar_luau::ModuleIdentity;

use instar_project::{
    PositionQuery, PositionQueryKind, Project, ProjectSnapshot, Query, QueryReport, QueryRequest,
    Task,
};

use instar_shared::{
    Availability, Completion, Knowledge, OperationFailure, Options, Reason, SourceSnapshot,
};

use tokio::task::JoinHandle;

use tower_lsp_server::{
    Client,
    gen_lsp_types::{Position, ProgressToken, Uri, WorkDoneProgressCancelParams},
    jsonrpc::{Error, ErrorCode, Result},
};

use super::{
    client::ProgressRecord,
    state::{Lifecycle, State},
};

use crate::position;

static SESSIONS: AtomicU64 = AtomicU64::new(1);

/// A protocol session over the project service.
pub struct Server {
    pub(crate) client: Client,
    pub(crate) project: Arc<Project>,
    pub(crate) options: Options,
    pub(crate) state: Arc<Mutex<State>>,
    pub(crate) publication: Mutex<Option<JoinHandle<()>>>,
    pub(crate) progress: Arc<Mutex<HashMap<ProgressToken, ProgressRecord>>>,
    pub(crate) refreshing: tokio::sync::Mutex<()>,
}

impl Server {
    /// Creates a session without starting analysis workers.
    #[must_use]
    pub fn new(client: Client, project: Arc<Project>, options: Options) -> Self {
        let state = State::new(SESSIONS.fetch_add(1, Ordering::Relaxed));

        Self {
            client,
            project,
            options,
            state: Arc::new(Mutex::new(state)),
            publication: Mutex::new(None),
            progress: Arc::new(Mutex::new(HashMap::new())),
            refreshing: tokio::sync::Mutex::new(()),
        }
    }

    pub(crate) fn capture(&self, uri: &Uri) -> Result<(ProjectSnapshot, SourceSnapshot)> {
        self.state
            .lock()
            .expect("session state")
            .capture(&self.project, uri)
    }

    pub(crate) fn position(
        &self,
        uri: &Uri,
        position: Position,
        kind: PositionQueryKind,
    ) -> Result<(ProjectSnapshot, PositionQuery)> {
        let state = self.state.lock().expect("session state");
        let (snapshot, source) = state.capture(&self.project, uri)?;
        let offset = position::offset(&source.text, position, &state.encoding)?;

        let module = ModuleIdentity {
            context: snapshot.context(source.version.identity),
            source: source.version.identity,
        };

        Ok((
            snapshot,
            PositionQuery {
                module,
                source: source.version,
                offset,
                kind,
            },
        ))
    }

    pub(crate) async fn query(
        &self,
        snapshot: ProjectSnapshot,
        operation: Query,
        token: Option<ProgressToken>,
    ) -> Result<QueryReport> {
        let partial = matches!(operation, Query::Diagnostics(_) | Query::Complete(_));
        let options = self.options.child();

        let task = self.project.query(
            QueryRequest {
                snapshot: snapshot.clone(),
                operation,
            },
            options.clone(),
        );

        let result = self
            .task(task, &options, token, "Processing Luau request")
            .await;

        let report = match result {
            Ok(report) => report,

            Err(error)
                if error.code != ErrorCode::RequestCancelled
                    && !self.state.lock().expect("session state").current(
                        &self.project,
                        &snapshot
                            .input_versions
                            .values()
                            .cloned()
                            .collect::<Vec<_>>(),
                    ) =>
            {
                return Err(Error::content_modified());
            }

            Err(error) => return Err(error),
        };

        if !self
            .state
            .lock()
            .expect("session state")
            .current(&self.project, &report.inputs)
        {
            return Err(Error::content_modified());
        }

        if report.completion != Completion::Complete && !partial {
            return Err(failure(&report.completion.error()));
        }

        if matches!(
            report.completion,
            Completion::Cancelled | Completion::TimedOut
        ) {
            return Err(failure(&report.completion.error()));
        }

        if let Availability::Unavailable(reason) = report.availability {
            return Err(if reason == Reason::StaleQuery {
                Error::content_modified()
            } else {
                failure(&reason.error())
            });
        }

        Ok(report)
    }

    pub(crate) async fn task<Output>(
        &self,
        task: Task<Output>,
        options: &Options,
        token: Option<ProgressToken>,
        title: &str,
    ) -> Result<Output> {
        let convert = |error: OperationFailure| {
            if matches!(&error.completion, Completion::Incomplete(reasons) if reasons.contains(&Reason::StaleQuery))
            {
                Error::content_modified()
            } else {
                failure(&io::Error::from(error))
            }
        };

        let mut task = Box::pin(task);

        let progress = tokio::select! {
            result = &mut task => return result.map_err(convert),
            progress = self.begin_progress(token, title, &options.cancellation) => progress?,
        };

        let result = task.await.map_err(convert);
        drop(progress);

        result
    }

    pub(crate) fn stop(&self, lifecycle: Lifecycle) {
        if let Some(publication) = self
            .publication
            .lock()
            .expect("diagnostic publication")
            .take()
        {
            publication.abort();
        }

        for record in self.progress.lock().expect("progress records").values() {
            record.cancellation.cancel();
        }

        self.project.stop();
        self.state.lock().expect("session state").lifecycle = lifecycle;
    }

    pub(crate) async fn cancel_progress(&self, parameters: WorkDoneProgressCancelParams) {
        if let Some(record) = self
            .progress
            .lock()
            .expect("progress records")
            .get(&parameters.token)
        {
            record.cancellation.cancel();
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(publication) = self
            .publication
            .lock()
            .expect("diagnostic publication")
            .take()
        {
            publication.abort();
        }

        for record in self.progress.lock().expect("progress records").values() {
            record.cancellation.cancel();
        }
    }
}

pub(crate) fn known<Output>(value: Knowledge<Output>) -> Result<Output> {
    match value {
        Knowledge::Known(value) => Ok(value),
        Knowledge::Unavailable(Reason::StaleQuery) => Err(Error::content_modified()),
        Knowledge::Unavailable(reason) => Err(failure(&reason.error())),
    }
}

pub(crate) fn failure(error: &io::Error) -> Error {
    let code = match error.kind() {
        io::ErrorKind::Interrupted => ErrorCode::RequestCancelled,
        io::ErrorKind::WouldBlock => ErrorCode::ServerError(-32802),
        io::ErrorKind::InvalidInput => ErrorCode::InvalidParams,
        _ => ErrorCode::ServerError(-32803),
    };

    Error {
        code,
        message: error.to_string().into(),
        data: None,
    }
}
