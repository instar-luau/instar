use std::{io, path::Path, time::Instant};

use instar_analysis::{Options, error::invalid};
use instar_core::{analysis::Entry, project::Project};

use tower_lsp_server::{
    gen_lsp_types::{
        Position, PositionEncodingKind, PrepareRenameParams, Range, RenameParams, WorkspaceEdit,
    },
    jsonrpc::Result,
};

use crate::{
    edits::{self, Edits},
    location, position,
    session::Session,
};

impl Session {
    pub(crate) async fn prepare_rename(
        &self,
        parameters: PrepareRenameParams,
    ) -> Result<Option<Range>> {
        let parameters = parameters.text_document_position_params;

        self.workspace
            .lock()
            .expect("workspace lock")
            .editable(&parameters.text_document.uri)?;

        self.query(
            &parameters.text_document.uri,
            &self.queries,
            move |path, encoding, project, options, started| {
                let (entry, offset) = entry(
                    path,
                    parameters.position,
                    encoding,
                    project,
                    options,
                    started,
                )?;

                project
                    .prepare_rename(&entry, offset, &options.remaining(started))?
                    .map(|reference| {
                        location::convert(&reference.location, project, encoding)
                            .map(|location| location.range)
                    })
                    .transpose()
            },
        )
        .await
    }

    pub(crate) async fn rename(&self, parameters: RenameParams) -> Result<WorkspaceEdit> {
        let parameters_position = parameters.text_document_position_params;

        let (path, rendering) = {
            let workspace = self.workspace.lock().expect("workspace lock");
            workspace.editable(&parameters_position.text_document.uri)?;
            let (_, path, _) = workspace.capture(&parameters_position.text_document.uri)?;

            (path, Edits::new(&workspace))
        };

        let edits = self
            .workspace_query(
                &self.queries,
                move |roots, encoding, project, options, started| {
                    let (entry, offset) = entry(
                        &path,
                        parameters_position.position,
                        encoding,
                        project,
                        options,
                        started,
                    )?;

                    let edits = project.rename(
                        &entry,
                        offset,
                        &parameters.new_name,
                        roots,
                        &options.remaining(started),
                    )?;

                    edits::convert(edits, project, encoding, options, started)
                },
            )
            .await?;

        let workspace = self.workspace.lock().expect("workspace lock");

        rendering.render(&workspace, edits)
    }
}

fn entry(
    path: &Path,
    position: Position,
    encoding: &PositionEncodingKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<(Entry, usize)> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, position, encoding)
        .map_err(|error| invalid(error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;

    let [entry] = entries.as_slice() else {
        return Err(invalid("rename requires one unambiguous module context"));
    };

    Ok((entry.clone(), offset))
}
