use std::{
    path::PathBuf,
    sync::{Arc, atomic::Ordering},
};

use tower_lsp_server::{
    gen_lsp_types::{ProgressToken, Uri},
    jsonrpc::Result,
};

use super::protocol::Service;

use crate::{
    analysis,
    connection::{Consistency, Priority, Target},
    files,
    output::messages,
};

impl Service {
    pub(super) async fn publish(&self, uri: Uri, should_report_progress: bool) -> Result<()> {
        let token = if should_report_progress && self.has_capability("/window/workDoneProgress") {
            let token = ProgressToken::String(format!(
                "instar/{}",
                self.sequence.fetch_add(1, Ordering::Relaxed)
            ));

            self.client.create_work_done_progress(token.clone()).await?;

            Some(token)
        } else {
            None
        };

        self.perform(
            Consistency::Current,
            Priority::Background,
            token,
            |operation| async move {
                let (batch, publications, snapshot) = self
                    .connection
                    .query(Target::Uri(uri), Arc::clone(&operation), |project, _| {
                        let (batch, publications) = analysis::prepare(project)?;

                        Ok((batch, publications, Box::new(project.capture())))
                    })
                    .await?;

                for publication in publications {
                    self.client
                        .publish_diagnostics(
                            publication.uri,
                            publication.diagnostics,
                            publication.version,
                        )
                        .await;
                }

                let publications = self
                    .connection
                    .query(
                        Target::Snapshot(snapshot),
                        operation,
                        move |project, frontend| analysis::check(project, frontend, batch),
                    )
                    .await?;

                for publication in publications {
                    self.client
                        .publish_diagnostics(
                            publication.uri,
                            publication.diagnostics,
                            publication.version,
                        )
                        .await;
                }

                Ok(())
            },
        )
        .await
    }

    pub(super) async fn revalidate(&self, should_report_progress: bool) -> Result<()> {
        let roots = self.connection.control(|state| state.roots()).await?;

        for root in roots {
            if let Err(error) = self.publish(root, should_report_progress).await {
                messages::report(&self.client, error).await;
            }
        }

        Ok(())
    }

    pub(super) async fn refresh(&self) {
        if self.has_capability("/workspace/semanticTokens/refreshSupport")
            && let Err(error) = self.client.semantic_tokens_refresh().await
        {
            messages::warning(&self.client, &error).await;
        }

        if self.has_capability("/workspace/inlayHint/refreshSupport")
            && let Err(error) = self.client.inlay_hint_refresh().await
        {
            messages::warning(&self.client, &error).await;
        }

        if self.has_capability("/workspace/foldingRange/refreshSupport")
            && let Err(error) = self.client.folding_range_refresh().await
        {
            messages::warning(&self.client, &error).await;
        }
    }

    pub(super) async fn maintain(&self, should_report_progress: bool) -> Result<()> {
        self.register().await?;
        self.refresh().await;
        self.revalidate(should_report_progress).await?;

        Ok(())
    }

    pub(super) async fn changed(&self, paths: Vec<PathBuf>) -> Result<()> {
        let has_changed = self
            .connection
            .control(move |state| files::changed(state, &paths))
            .await?;

        if !has_changed {
            return Ok(());
        }

        self.maintain(false).await
    }
}
