use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};

use instar_bridge::Progress as NativeProgress;
use serde_json::json;
use tokio::runtime::Handle;

use tower_lsp_server::{
    Client,
    gen_lsp_types::{ProgressNotification, ProgressParams, ProgressToken},
};

use crate::connection::Operation;

pub(crate) type Registry = Arc<Mutex<HashMap<ProgressToken, Weak<Operation>>>>;

pub(crate) struct Progress {
    client: Client,
    token: Option<ProgressToken>,
    registry: Registry,
    handle: Handle,
}

impl Progress {
    pub(crate) fn new(
        client: Client,
        token: Option<ProgressToken>,
        operation: &Arc<Operation>,
        registry: Registry,
    ) -> Self {
        if let Some(token) = &token {
            registry
                .lock()
                .expect("Internal error: progress registry mutex is poisoned.")
                .insert(token.clone(), Arc::downgrade(operation));
        }

        Self {
            client,
            token,
            registry,
            handle: Handle::current(),
        }
    }

    pub(crate) async fn begin(&self) {
        if let Some(token) = &self.token {
            self.client
                .send_notification::<ProgressNotification>(ProgressParams {
                    token: token.clone(),
                    value: json!({"kind":"begin","title":"Processing request","cancellable":true}),
                })
                .await;
        }
    }

    pub(crate) async fn finish(mut self) {
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
            .expect("Internal error: progress registry mutex is poisoned.")
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

pub(crate) fn report(
    client: &Client,
    token: &ProgressToken,
    handle: &Handle,
    stage: NativeProgress<'_>,
) {
    let message = match stage {
        NativeProgress::Started(module) | NativeProgress::Finished(module) => module.to_owned(),

        NativeProgress::Advanced { completed, total } => {
            let unit = if total == 1 { "module" } else { "modules" };

            format!("{completed} of {total} {unit}")
        }
    };

    handle.block_on(
        client.send_notification::<ProgressNotification>(ProgressParams {
            token: token.clone(),
            value: json!({"kind":"report","cancellable":true,"message":message}),
        }),
    );
}
