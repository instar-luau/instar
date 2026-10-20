use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use instar_bridge::{Cancellation, Progress};
use tokio::runtime::Handle;
use tower_lsp_server::{Client, gen_lsp_types::ProgressToken};

use crate::output::progress;

pub(crate) struct Operation {
    is_cancelled: AtomicBool,
    native: Mutex<Option<Cancellation>>,
    progress: Option<(Client, ProgressToken, Handle)>,
}

impl Operation {
    pub(crate) fn new(progress: Option<(Client, ProgressToken)>) -> Self {
        Self {
            is_cancelled: AtomicBool::new(false),
            native: Mutex::new(None),
            progress: progress.map(|(client, token)| (client, token, Handle::current())),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.is_cancelled.load(Ordering::Relaxed)
    }

    pub(crate) fn cancel(&self) {
        self.is_cancelled.store(true, Ordering::Relaxed);

        if let Some(native) = &*self
            .native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.")
        {
            native.cancel();
        }
    }

    pub(crate) fn attach(&self, cancellation: &Cancellation) {
        let mut native = self
            .native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.");

        cancellation.reset();
        *native = Some(cancellation.clone());

        if self.is_cancelled() {
            cancellation.cancel();
        }
    }

    pub(crate) fn detach(&self) {
        self.native
            .lock()
            .expect("Internal error: cancellation handle mutex is poisoned.")
            .take();
    }

    pub(crate) fn report(&self, stage: Progress<'_>) -> bool {
        if self.is_cancelled() {
            return false;
        }

        if let Some((client, token, handle)) = &self.progress {
            progress::report(client, token, handle, stage);
        }

        true
    }
}

pub(super) struct Guard(pub(super) Option<Arc<Operation>>);

impl Drop for Guard {
    fn drop(&mut self) {
        if let Some(operation) = &self.0 {
            operation.cancel();
        }
    }
}
