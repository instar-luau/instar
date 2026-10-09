use std::{io, path::Path, sync::Arc, time::Instant};

use instar_analysis::Options;
use instar_core::{analysis::Origin, project::Project};

use tower_lsp_server::{
    gen_lsp_types::{
        DefinitionParams, Location, Position, PositionEncodingKind, TextDocumentPositionParams,
        TypeDefinitionParams,
    },
    jsonrpc::Result,
};

use crate::{location, position, session::Session, workspace};

pub(crate) struct Target {
    pub(crate) location: Location,
    pub(crate) content: Option<Arc<str>>,
}

impl Session {
    pub(crate) async fn definition(&self, parameters: DefinitionParams) -> Result<Vec<Location>> {
        self.navigate(parameters.text_document_position_params, false)
            .await
    }

    pub(crate) async fn type_definition(
        &self,
        parameters: TypeDefinitionParams,
    ) -> Result<Vec<Location>> {
        self.navigate(parameters.text_document_position_params, true)
            .await
    }

    pub(crate) fn locations(&self, targets: Vec<Target>) -> Result<Vec<Location>> {
        let mut workspace = self.workspace.lock().expect("workspace lock");
        let locations = workspace.locations();

        targets
            .into_iter()
            .map(|mut target| {
                if let Some(content) = target.content {
                    target.location.uri = workspace.definition(target.location.uri, content)?;
                } else if let Some(uri) = locations.get(&workspace::path(&target.location.uri)?) {
                    target.location.uri = uri.clone();
                }

                Ok(target.location)
            })
            .collect()
    }

    async fn navigate(
        &self,
        parameters: TextDocumentPositionParams,
        type_definition: bool,
    ) -> Result<Vec<Location>> {
        let targets = self
            .query(
                &parameters.text_document.uri,
                &self.queries,
                move |path, encoding, project, options, started| {
                    query(
                        path,
                        parameters.position,
                        encoding,
                        project,
                        type_definition,
                        options,
                        started,
                    )
                },
            )
            .await?;

        self.locations(targets)
    }
}

fn query(
    path: &Path,
    position: Position,
    encoding: &PositionEncodingKind,
    project: &mut Project,
    type_definition: bool,
    options: &Options,
    started: Instant,
) -> io::Result<Vec<Target>> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, position, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;
    let mut targets: Vec<Target> = Vec::new();

    for entry in entries {
        for definition in
            project.definition(&entry, offset, type_definition, &options.remaining(started))?
        {
            let location = location::convert(&definition, project, encoding)?;

            if targets.iter().any(|target| target.location == location) {
                continue;
            }

            let content = if matches!(definition.module, Origin::Definition(_)) {
                Some(project.source(definition.module.source())?.text)
            } else {
                None
            };

            targets.push(Target { location, content });
        }
    }

    options.check(started)?;

    Ok(targets)
}
