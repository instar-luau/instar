use std::{
    io,
    path::{Path, PathBuf},
    time::Instant,
};

use instar_analysis::Options;
use instar_core::{analysis::Origin, project::Project};

use tower_lsp_server::{
    gen_lsp_types::{
        DocumentHighlight, DocumentHighlightKind, DocumentHighlightParams, Location, Position,
        PositionEncodingKind, ReferenceParams,
    },
    jsonrpc::Result,
};

use crate::{location, navigation::Target, position, session::Session};

struct Occurrence {
    target: Target,
    declaration: bool,
    write: bool,
}

impl Session {
    pub(crate) async fn references(&self, parameters: ReferenceParams) -> Result<Vec<Location>> {
        let (_, path, _) = self
            .workspace
            .lock()
            .expect("workspace lock")
            .capture(&parameters.text_document_position_params.text_document.uri)?;

        let references = self
            .workspace_query(
                &self.queries,
                move |roots, encoding, project, options, started| {
                    query(
                        &path,
                        parameters.text_document_position_params.position,
                        roots,
                        encoding,
                        project,
                        options,
                        started,
                    )
                },
            )
            .await?;

        self.locations(
            references
                .into_iter()
                .filter(|reference| {
                    parameters.context.include_declaration || !reference.declaration
                })
                .map(|reference| reference.target)
                .collect(),
        )
    }

    pub(crate) async fn highlights(
        &self,
        parameters: DocumentHighlightParams,
    ) -> Result<Vec<DocumentHighlight>> {
        self.query(
            &parameters.text_document_position_params.text_document.uri,
            &self.queries,
            move |path, encoding, project, options, started| {
                let uri = location::uri(path)?;

                Ok(query(
                    path,
                    parameters.text_document_position_params.position,
                    &[],
                    encoding,
                    project,
                    options,
                    started,
                )?
                .into_iter()
                .filter(|reference| reference.target.location.uri == uri)
                .map(|reference| DocumentHighlight {
                    range: reference.target.location.range,
                    kind: Some(if reference.write {
                        DocumentHighlightKind::Write
                    } else {
                        DocumentHighlightKind::Read
                    }),
                })
                .collect())
            },
        )
        .await
    }
}

fn query(
    path: &Path,
    position: Position,
    roots: &[PathBuf],
    encoding: &PositionEncodingKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<Occurrence>> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, position, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;
    let mut occurrences: Vec<Occurrence> = Vec::new();

    for entry in entries {
        for reference in project.references(&entry, offset, roots, &options.remaining(started))? {
            options.check(started)?;
            let location = location::convert(&reference.location, project, encoding)?;

            if let Some(existing) = occurrences
                .iter_mut()
                .find(|occurrence| occurrence.target.location == location)
            {
                existing.write |= reference.write;
                existing.declaration |= reference.declaration;
                continue;
            }

            let content = if matches!(reference.location.module, Origin::Definition(_)) {
                Some(project.source(reference.location.module.source())?.text)
            } else {
                None
            };

            occurrences.push(Occurrence {
                target: Target { location, content },
                declaration: reference.declaration,
                write: reference.write,
            });
        }
    }

    options.check(started)?;

    Ok(occurrences)
}
