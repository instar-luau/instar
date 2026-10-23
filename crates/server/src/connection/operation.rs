use std::sync::{Arc, Mutex};

use instar_bridge::{Cancellation, Progress};
use tokio::runtime::Handle;
use tokio_util::sync::CancellationToken;
use tower_lsp_server::{Client, gen_lsp_types::ProgressToken};

use crate::output::progress;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Consistency {
    Snapshot,
    Current,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Priority {
    Interactive,
    Background,
}

pub(crate) struct Operation {
    pub(super) consistency: Consistency,
    pub(super) priority: Priority,
    pub(super) cancellation: CancellationToken,
    native: Mutex<Option<Cancellation>>,
    progress: Option<(Client, ProgressToken, Handle)>,
}

impl Operation {
    pub(crate) fn new(
        consistency: Consistency,
        priority: Priority,
        progress: Option<(Client, ProgressToken)>,
    ) -> Self {
        Self {
            consistency,
            priority,
            cancellation: CancellationToken::new(),
            native: Mutex::new(None),
            progress: progress.map(|(client, token)| (client, token, Handle::current())),
        }
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();

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
