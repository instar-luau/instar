use std::{io, sync::Arc};

use instar_project::Project;
use instar_shared::Options;

use tokio::{
    io::{AsyncRead, AsyncWrite, stdin, stdout},
    sync::Notify,
};

use tower_lsp_server::{LspService, Server as LanguageServerTransport};

use super::{Dispatch, connection::Connection};
use crate::{Server, session::Lifecycle};

/// Connection termination and project cleanup outcomes.
#[derive(Debug)]
pub struct ServerOutcome {
    /// Whether the client requested orderly shutdown.
    pub shutdown_requested: bool,

    /// Whether an exit notification was received.
    pub exit_received: bool,

    /// Whether the connection ended without exit.
    pub disconnected: bool,

    /// Whether project workers were successfully reaped.
    pub cleanup: io::Result<()>,
}

/// Serves one connection using the supplied project service.
///
/// # Errors
/// Returns transport setup errors; cleanup failures remain in the outcome.
///
/// # Panics
/// Panics if session state was poisoned by a previous panic.
pub async fn serve<Input, Output>(
    input: Input,
    output: Output,
    project: Arc<Project>,
    options: Options,
) -> io::Result<ServerOutcome>
where
    Input: AsyncRead + Unpin + Send + 'static,
    Output: AsyncWrite + Unpin + Send + 'static,
{
    let disconnected = Arc::new(Notify::new());

    let (service, socket) =
        LspService::build(|client| Server::new(client, Arc::clone(&project), options))
            .custom_method("window/workDoneProgress/cancel", Server::cancel_progress)
            .finish();

    let state = Arc::clone(&service.inner().state);
    let dispatch = Dispatch { service };

    let input = Connection {
        stream: input,
        disconnected: Arc::clone(&disconnected),
    };

    let output = Connection {
        stream: output,
        disconnected: Arc::clone(&disconnected),
    };

    tokio::select! {
        () = LanguageServerTransport::new(input, output, socket).concurrency_level(usize::MAX).serve(dispatch) => {},
        () = disconnected.notified() => {},
    }

    project.stop();
    let cleanup = project.finish().await;
    let mut state = state.lock().expect("session state");
    let disconnected = !state.exit_received;

    if disconnected {
        state.lifecycle = Lifecycle::Disconnected;
    }

    Ok(ServerOutcome {
        shutdown_requested: state.shutdown_requested,
        exit_received: state.exit_received,
        disconnected,
        cleanup,
    })
}

/// Serves process stdio using the supplied project service.
///
/// # Errors
/// Returns transport setup errors; cleanup failures remain in the outcome.
///
/// # Panics
/// Panics if session state was poisoned by a previous panic.
pub async fn stdio(project: Arc<Project>, options: Options) -> io::Result<ServerOutcome> {
    serve(stdin(), stdout(), project, options).await
}
