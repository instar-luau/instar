use std::{
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::Options;
use instar_core::{project::Project, snapshot::Snapshot};
use tokio::time;

use tower_lsp_server::{
    gen_lsp_types::{PositionEncodingKind, Uri},
    jsonrpc::{Error, Result},
};

use crate::{
    session::{Session, failure},
    workers::{Output, Workers},
};

impl Session {
    pub(crate) async fn query<Response: Send + 'static>(
        &self,
        uri: &Uri,
        workers: &Workers,
        operation: impl FnOnce(
            &Path,
            &PositionEncodingKind,
            &mut Project,
            &Options,
            Instant,
        ) -> io::Result<Response>
        + Send
        + 'static,
    ) -> Result<Response> {
        let (snapshot, path, encoding) = self
            .workspace
            .lock()
            .expect("workspace lock")
            .capture(uri)?;

        self.execute(snapshot, workers, move |project, options, started| {
            operation(&path, &encoding, project, options, started)
        })
        .await
    }

    pub(crate) async fn workspace_query<Response: Send + 'static>(
        &self,
        workers: &Workers,
        operation: impl FnOnce(
            &[PathBuf],
            &PositionEncodingKind,
            &mut Project,
            &Options,
            Instant,
        ) -> io::Result<Response>
        + Send
        + 'static,
    ) -> Result<Response> {
        let (snapshot, roots, encoding) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            (
                workspace.snapshot.clone(),
                workspace.index_roots(),
                workspace.encoding.clone(),
            )
        };

        let revision = snapshot.revision();

        let result = self
            .execute(snapshot, workers, move |project, options, started| {
                operation(&roots, &encoding, project, options, started)
            })
            .await?;

        if self
            .workspace
            .lock()
            .expect("workspace lock")
            .snapshot
            .revision()
            != revision
        {
            return Err(Error::content_modified());
        }

        Ok(result)
    }

    pub(crate) async fn execute<Response: Send + 'static>(
        &self,
        snapshot: Snapshot,
        workers: &Workers,
        operation: impl FnOnce(&mut Project, &Options, Instant) -> io::Result<Response> + Send + 'static,
    ) -> Result<Response> {
        let options = Options {
            cancellation: self.options.cancellation.child(),
            ..self.options.clone()
        };

        let timeout = options.timeout;

        let (_guard, response) = workers
            .submit(
                snapshot.clone(),
                options,
                move |project, _, options, started| {
                    let result = operation(project, options, started);

                    Output {
                        result,
                        inputs: project.watch_inputs(),
                    }
                },
            )
            .map_err(|error| failure(&error))?;

        let output = time::timeout(timeout, response)
            .await
            .map_err(|_| {
                failure(&io::Error::new(
                    io::ErrorKind::TimedOut,
                    "query deadline expired",
                ))
            })?
            .map_err(|_| failure(&io::Error::other("query worker stopped")))?;

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .current(&snapshot, &output.inputs)
        {
            return Err(Error::content_modified());
        }

        output.result.map_err(|error| {
            if error.kind() == io::ErrorKind::InvalidInput {
                Error::invalid_params(error.to_string())
            } else {
                failure(&error)
            }
        })
    }
}
