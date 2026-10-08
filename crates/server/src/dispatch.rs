use std::{
    future::{Future, ready},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};

use tower_lsp_server::{
    ExitedError, LspService,
    jsonrpc::{Request, Response},
};

use tower_service::Service;

use crate::session::Session;

pub(crate) struct Dispatch {
    pub(crate) service: LspService<Session>,
    pub(crate) exited: Arc<AtomicBool>,
}

impl Service<Request> for Dispatch {
    type Response = Option<Response>;
    type Error = ExitedError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.service.poll_ready(context)
    }

    fn call(&mut self, request: Request) -> Self::Future {
        if request.method() == "exit" && request.id().is_none() {
            self.exited.store(true, Ordering::Relaxed);
            self.service.inner().workers.stop();
        }

        let mut response = self.service.call(request);

        match response
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            Poll::Ready(response) => Box::pin(ready(response)),
            Poll::Pending => response,
        }
    }
}
