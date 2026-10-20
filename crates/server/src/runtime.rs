use std::{
    future,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use eyre::{Result, WrapErr as _};
use tokio::{io, runtime::Builder, time};

use tower_lsp_server::{
    ExitedError, LspService, Server,
    jsonrpc::{FromParams as _, Request, Response},
};

use tower_service::Service as RequestService;

use super::{connection::Connection, service::Service};

struct Transport(LspService<Service>);

impl RequestService<Request> for Transport {
    type Response = Option<Response>;
    type Error = ExitedError;
    type Future = <LspService<Service> as RequestService<Request>>::Future;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        if request.method() == "shutdown"
            && let Err(error) = <()>::from_params(request.params().cloned())
        {
            let response = request
                .id()
                .cloned()
                .map(|id| Response::from_error(id, error));

            return Box::pin(future::ready(Ok(response)));
        }

        self.0.call(request)
    }
}

/// Run the language server over standard input and standard output.
///
/// # Errors
/// Returns runtime initialization or analysis worker shutdown failures.
pub fn run(is_cancelled: fn() -> bool) -> Result<u8> {
    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .wrap_err("Could not create the server runtime.")?;

    let connection = Arc::new(Connection::new().wrap_err("Could not start the analysis worker.")?);
    let has_shutdown = Arc::new(AtomicBool::new(false));

    runtime.block_on(async {
        let connection = Arc::clone(&connection);
        let has_shutdown = Arc::clone(&has_shutdown);

        let (service, socket) =
            LspService::build(move |client| Service::new(client, connection, has_shutdown))
                .custom_method(
                    "window/workDoneProgress/cancel",
                    |service: &Service, parameters| {
                        service.cancel_progress(&parameters);

                        future::ready(())
                    },
                )
                .finish();

        let serving = Server::new(io::stdin(), io::stdout(), socket)
            .concurrency_level(usize::MAX)
            .serve(Transport(service));

        tokio::pin!(serving);
        let mut interval = time::interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                () = &mut serving => break,
                _ = interval.tick() => if is_cancelled() { break; },
            }
        }
    });

    let stopped = connection.stop();
    runtime.shutdown_background();
    stopped.wrap_err("Could not stop the analysis worker.")?;

    Ok(u8::from(
        !has_shutdown.load(Ordering::Relaxed) && !is_cancelled(),
    ))
}
