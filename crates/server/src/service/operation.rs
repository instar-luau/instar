use std::{future::Future, ops::ControlFlow, sync::Arc};

use instar_bridge::Frontend;

use tower_lsp_server::{
    gen_lsp_types::{ProgressToken, Range, TextEdit, Uri},
    jsonrpc::Result,
};

use super::protocol::Service;

use crate::{
    connection::{Consistency, Operation, Priority, Target},
    error::failure,
    output::{messages, progress::Progress},
    project::Project,
};

impl Service {
    pub(super) async fn perform<Output, Pending>(
        &self,
        consistency: Consistency,
        priority: Priority,
        token: Option<ProgressToken>,
        execute: impl FnOnce(Arc<Operation>) -> Pending + Send,
    ) -> Result<Output>
    where
        Output: Send,
        Pending: Future<Output = Result<Output>> + Send,
    {
        let operation = Arc::new(Operation::new(
            consistency,
            priority,
            token.clone().map(|token| (self.client.clone(), token)),
        ));

        let progress = Progress::new(
            self.client.clone(),
            token,
            &operation,
            Arc::clone(&self.progress),
        );

        progress.begin().await;
        let result = execute(operation).await;
        progress.finish().await;

        result
    }

    pub(super) async fn query<Parameters, Output>(
        &self,
        consistency: Consistency,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, &mut Frontend<'_>, Parameters) -> Result<Output>
        + Send
        + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(consistency, Priority::Interactive, token, |operation| {
            self.connection
                .query(Target::Uri(uri), operation, move |project, frontend| {
                    execute(project, frontend, parameters)
                })
        })
        .await
    }

    pub(super) async fn source<Parameters, Output>(
        &self,
        consistency: Consistency,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, Parameters) -> Result<Output> + Send + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(consistency, Priority::Interactive, token, |operation| {
            self.connection
                .source(uri, operation, move |project| execute(project, parameters))
        })
        .await
    }

    pub(super) async fn source_or_query<Parameters, Output>(
        &self,
        consistency: Consistency,
        uri: Uri,
        token: Option<ProgressToken>,
        parameters: Parameters,
        source: impl FnOnce(&mut Project, &Parameters) -> Result<ControlFlow<Output>> + Send + 'static,
        query: impl FnOnce(&mut Project, &mut Frontend<'_>, Parameters) -> Result<Output>
        + Send
        + 'static,
    ) -> Result<Output>
    where
        Parameters: Send + 'static,
        Output: Send + 'static,
    {
        self.perform(
            consistency,
            Priority::Interactive,
            token,
            |operation| async move {
                let prepared = self
                    .connection
                    .source(
                        uri.clone(),
                        Arc::clone(&operation),
                        move |project| match source(project, &parameters)? {
                            ControlFlow::Break(output) => Ok(ControlFlow::Break(output)),

                            ControlFlow::Continue(()) => {
                                let module = project.module(&uri)?;
                                let document = project.snapshot(&module)?;
                                let mut snapshot = project.capture();

                                if document.version.is_none() {
                                    snapshot.borrowed.insert(module, document);
                                }

                                Ok(ControlFlow::Continue((parameters, Box::new(snapshot))))
                            }
                        },
                    )
                    .await?;

                let (parameters, snapshot) = match prepared {
                    ControlFlow::Break(output) => return Ok(output),
                    ControlFlow::Continue(prepared) => prepared,
                };

                self.connection
                    .query(
                        Target::Snapshot(snapshot),
                        operation,
                        move |project, frontend| query(project, frontend, parameters),
                    )
                    .await
            },
        )
        .await
    }

    pub(super) async fn format(
        &self,
        uri: Uri,
        token: Option<ProgressToken>,
        range: Option<Range>,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.perform(
            Consistency::Current,
            Priority::Interactive,
            token,
            |operation| async move {
                let permit = Arc::clone(&self.formatting)
                    .acquire_owned()
                    .await
                    .map_err(failure)?;

                self.connection
                    .formatting(uri, operation, permit, range)
                    .await
            },
        )
        .await
    }

    pub(super) async fn notify<Parameters>(
        &self,
        uri: Uri,
        parameters: Parameters,
        execute: impl FnOnce(&mut Project, Parameters) -> Result<()> + Send + 'static,
    ) where
        Parameters: Send + 'static,
    {
        let result = async {
            self.connection
                .update(uri.clone(), move |project| execute(project, parameters))
                .await?;

            self.publish(uri, false).await
        }
        .await;

        if let Err(error) = result {
            messages::report(&self.client, error).await;
        }
    }
}
