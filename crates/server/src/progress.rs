use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use serde_json::json;
use tokio::runtime::Handle;

use tower_lsp_server::{
    Client,
    gen_lsp_types::{ProgressNotification, ProgressParams, ProgressToken},
};

use super::connection::Operation;

pub(super) type Registry = Arc<Mutex<HashMap<ProgressToken, Weak<Operation>>>>;

pub(super) struct Progress {
    client: Client,
    token: Option<ProgressToken>,
    registry: Registry,
    handle: Handle,
}

impl Progress {
    pub(super) fn new(
        client: Client,
        token: Option<ProgressToken>,
        operation: &Arc<Operation>,
        registry: Registry,
    ) -> Self {
        if let Some(token) = &token {
            registry
                .lock()
                .expect("internal error: progress registry poisoned")
                .insert(token.clone(), Arc::downgrade(operation));
        }

        Self {
            client,
            token,
            registry,
            handle: Handle::current(),
        }
    }

    pub(super) async fn begin(&self) {
        if let Some(token) = &self.token {
            self.client
                .send_notification::<ProgressNotification>(ProgressParams {
                    token: token.clone(),
                    value: json!({"kind":"begin","title":"Analyzing","cancellable":true}),
                })
                .await;
        }
    }

    pub(super) async fn finish(mut self) {
        if let Some(token) = self.take() {
            self.client
                .send_notification::<ProgressNotification>(ProgressParams {
                    token,
                    value: json!({"kind":"end"}),
                })
                .await;
        }
    }

    fn take(&mut self) -> Option<ProgressToken> {
        let token = self.token.take()?;

        self.registry
            .lock()
            .expect("internal error: progress registry poisoned")
            .remove(&token);

        Some(token)
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        if let Some(token) = self.take() {
            let client = self.client.clone();

            drop(self.handle.spawn(async move {
                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token,
                        value: json!({"kind":"end"}),
                    })
                    .await;
            }));
        }
    }
}
