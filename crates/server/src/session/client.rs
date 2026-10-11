use std::{
    collections::HashMap,
    future::Future,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use instar_shared::{Cancellation, Completion, Options};
use serde_json::json;
use tokio::{runtime::Handle, time::timeout_at};

use tower_lsp_server::{
    Client,
    gen_lsp_types::{
        ApplyWorkspaceEditParams, ApplyWorkspaceEditRequest, ApplyWorkspaceEditResult, MessageType,
        ProgressNotification, ProgressParams, ProgressToken, Registration, Unregistration,
        WorkspaceEdit,
    },
    jsonrpc::{Error, Result},
};

use super::{Server, failure};

static PROGRESS: AtomicU64 = AtomicU64::new(1);

pub(crate) struct ProgressRecord {
    pub(crate) cancellation: Cancellation,
}

pub(crate) struct ProgressGuard {
    client: Client,
    token: ProgressToken,
    records: Arc<Mutex<HashMap<ProgressToken, ProgressRecord>>>,
    started: bool,
    created: bool,
}

impl ProgressGuard {
    pub(crate) async fn begin(mut self, title: &str, options: &Options) -> Result<Self> {
        options.check().map_err(|error| failure(&error))?;

        if self.created {
            let request = self.client.create_work_done_progress(self.token.clone());

            match options.deadline {
                Some(deadline) => {
                    timeout_at(deadline.into(), request)
                        .await
                        .map_err(|error| Error {
                            message: format!("{}: {error}", Completion::TimedOut.error()).into(),
                            ..failure(&Completion::TimedOut.error())
                        })??;
                }

                None => request.await?,
            }
        }

        options.check().map_err(|error| failure(&error))?;

        drop(
            self.client
                .progress(self.token.clone(), title)
                .with_cancel_button()
                .begin()
                .await,
        );

        self.started = true;
        options.check().map_err(|error| failure(&error))?;

        Ok(self)
    }
}

impl Drop for ProgressGuard {
    fn drop(&mut self) {
        if self
            .records
            .lock()
            .expect("progress records")
            .remove(&self.token)
            .is_none()
            || !self.started
        {
            return;
        }

        let client = self.client.clone();
        let token = self.token.clone();

        if let Ok(runtime) = Handle::try_current() {
            runtime.spawn(async move {
                client
                    .send_notification::<ProgressNotification>(ProgressParams {
                        token,
                        value: json!({"kind":"end"}),
                    })
                    .await;
            });
        }
    }
}

impl Server {
    pub(crate) async fn client_request<Output>(
        &self,
        request: impl Future<Output = Result<Output>>,
    ) -> Result<Output> {
        self.options.check().map_err(|error| failure(&error))?;

        match self.options.deadline {
            Some(deadline) => timeout_at(deadline.into(), request)
                .await
                .map_err(|error| Error {
                    message: format!("{}: {error}", Completion::TimedOut.error()).into(),
                    ..failure(&Completion::TimedOut.error())
                })?,

            None => request.await,
        }
    }

    pub(crate) fn prepare_progress(
        &self,
        supplied: Option<ProgressToken>,
        cancellation: &Cancellation,
    ) -> Result<Option<ProgressGuard>> {
        let supported = self
            .state
            .lock()
            .expect("session state")
            .capabilities
            .client
            .window
            .as_ref()
            .is_some_and(|window| window.work_done_progress == Some(true));

        let created = supplied.is_none();

        let token = match supplied {
            Some(token) => token,

            None if supported => format!(
                "instar-progress-{}",
                PROGRESS.fetch_add(1, Ordering::Relaxed)
            )
            .into(),

            None => return Ok(None),
        };

        {
            let mut records = self.progress.lock().expect("progress records");

            if records.contains_key(&token) {
                return Err(Error::invalid_params("progress token is already active"));
            }

            records.insert(
                token.clone(),
                ProgressRecord {
                    cancellation: cancellation.clone(),
                },
            );
        }

        Ok(Some(ProgressGuard {
            client: self.client.clone(),
            token,
            records: Arc::clone(&self.progress),
            started: false,
            created,
        }))
    }

    pub(crate) async fn register(&self, registration: Registration) -> Result<()> {
        self.client_request(self.client.register_capability(vec![registration.clone()]))
            .await?;

        self.state
            .lock()
            .expect("session state")
            .registrations
            .insert(registration.id.clone(), registration);

        Ok(())
    }

    pub(crate) async fn unregister(&self) -> Result<()> {
        let registrations = self
            .state
            .lock()
            .expect("session state")
            .registrations
            .values()
            .map(|registration| Unregistration {
                id: registration.id.clone(),
                method: registration.method.clone(),
            })
            .collect::<Vec<_>>();

        if registrations.is_empty() {
            return Ok(());
        }

        self.client_request(self.client.unregister_capability(registrations.clone()))
            .await?;

        let mut state = self.state.lock().expect("session state");

        for registration in registrations {
            state.registrations.remove(&registration.id);
        }

        Ok(())
    }

    pub(crate) async fn apply_edit(
        &self,
        label: String,
        edit: WorkspaceEdit,
    ) -> Result<ApplyWorkspaceEditResult> {
        if self
            .state
            .lock()
            .expect("session state")
            .capabilities
            .workspace()
            .is_none_or(|workspace| workspace.apply_edit != Some(true))
        {
            return Err(Error::invalid_params(
                "client does not support workspace edits",
            ));
        }

        let result = self
            .client_request(self.client.send_request::<ApplyWorkspaceEditRequest>(
                ApplyWorkspaceEditParams {
                    label: Some(label),
                    edit,
                    metadata: None,
                },
            ))
            .await?;

        let mut state = self.state.lock().expect("session state");
        state.edit_outcomes.push(result.clone());

        if state.edit_outcomes.len() > 32 {
            state.edit_outcomes.remove(0);
        }

        Ok(result)
    }

    pub(crate) async fn report(&self, result: Result<()>) {
        if let Err(error) = result {
            self.client
                .log_message(MessageType::Error, error.to_string())
                .await;

            self.client
                .show_message(MessageType::Error, error.to_string())
                .await;
        }
    }

    pub(crate) async fn refresh(&self) {
        let Ok(_guard) = self.refreshing.try_lock() else {
            return;
        };

        let supported = {
            let state = self.state.lock().expect("session state");

            let workspace = state.capabilities.workspace();

            [
                workspace
                    .and_then(|workspace| workspace.diagnostics.as_ref())
                    .is_some_and(|diagnostics| diagnostics.refresh_support == Some(true)),
                workspace
                    .and_then(|workspace| workspace.semantic_tokens.as_ref())
                    .is_some_and(|tokens| tokens.refresh_support == Some(true)),
                workspace
                    .and_then(|workspace| workspace.inlay_hint.as_ref())
                    .is_some_and(|hints| hints.refresh_support == Some(true)),
                workspace
                    .and_then(|workspace| workspace.code_lens.as_ref())
                    .is_some_and(|lenses| lenses.refresh_support == Some(true)),
                workspace
                    .and_then(|workspace| workspace.inline_value.as_ref())
                    .is_some_and(|values| values.refresh_support == Some(true)),
                workspace
                    .and_then(|workspace| workspace.folding_range.as_ref())
                    .is_some_and(|ranges| ranges.refresh_support == Some(true)),
            ]
        };

        if supported[0] {
            self.report(
                self.client_request(self.client.workspace_diagnostic_refresh())
                    .await,
            )
            .await;
        }

        if supported[1] {
            self.report(
                self.client_request(self.client.semantic_tokens_refresh())
                    .await,
            )
            .await;
        }

        if supported[2] {
            self.report(self.client_request(self.client.inlay_hint_refresh()).await)
                .await;
        }

        if supported[3] {
            self.report(self.client_request(self.client.code_lens_refresh()).await)
                .await;
        }

        if supported[4] {
            self.report(
                self.client_request(self.client.inline_value_refresh())
                    .await,
            )
            .await;
        }

        if supported[5] {
            self.report(
                self.client_request(self.client.folding_range_refresh())
                    .await,
            )
            .await;
        }

        let documents = {
            let state = self.state.lock().expect("session state");

            if state
                .capabilities
                .workspace()
                .is_some_and(|workspace| workspace.text_document_content.is_some())
            {
                state
                    .documents
                    .keys()
                    .filter(|uri| matches!(uri.scheme(), "instar" | "untitled"))
                    .cloned()
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            }
        };

        for uri in documents {
            self.report(
                self.client_request(self.client.text_document_content_refresh(uri))
                    .await,
            )
            .await;
        }
    }
}
