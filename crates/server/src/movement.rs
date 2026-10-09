use tower_lsp_server::{
    gen_lsp_types::{RenameFilesParams, WorkspaceEdit},
    jsonrpc::Result,
};

use crate::{
    edits::{self, Edits},
    session::Session,
    workspace,
};

impl Session {
    pub(crate) async fn move_imports(
        &self,
        parameters: RenameFilesParams,
    ) -> Result<WorkspaceEdit> {
        let (rendering, movements) = {
            let workspace = self.workspace.lock().expect("workspace lock");

            let movements = parameters
                .files
                .into_iter()
                .map(|file| {
                    workspace.editable(&file.old_uri)?;
                    workspace.editable(&file.new_uri)?;

                    Ok((
                        workspace::path(&file.old_uri)?,
                        workspace::path(&file.new_uri)?,
                    ))
                })
                .collect::<Result<Vec<_>>>()?;

            (Edits::new(&workspace), movements)
        };

        let edits = self
            .workspace_query(
                &self.workers,
                move |roots, encoding, project, options, started| {
                    let edits = project.relocate(&movements, roots, &options.remaining(started))?;

                    edits::convert(edits, project, encoding, options, started)
                },
            )
            .await?;

        rendering.render(&self.workspace.lock().expect("workspace lock"), edits)
    }
}
