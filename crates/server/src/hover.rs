use std::{collections::BTreeSet, io, path::Path, time::Instant};

use instar_analysis::Options;
use instar_core::project::Project;

use tower_lsp_server::{
    gen_lsp_types::{
        Hover, HoverParams, MarkupContent, MarkupKind, Position, PositionEncodingKind,
    },
    jsonrpc::Result,
};

use crate::{markup::code, position, session::Session};

impl Session {
    pub(crate) async fn hover(&self, parameters: HoverParams) -> Result<Option<Hover>> {
        let parameters = parameters.text_document_position_params;

        let markup = self
            .workspace
            .lock()
            .expect("workspace lock")
            .hover_format
            .clone();

        self.query(
            &parameters.text_document.uri,
            &self.queries,
            move |path, encoding, project, options, started| {
                query(
                    path,
                    parameters.position,
                    encoding,
                    &markup,
                    project,
                    options,
                    started,
                )
            },
        )
        .await
    }
}

fn query(
    path: &Path,
    position: Position,
    encoding: &PositionEncodingKind,
    markup: &MarkupKind,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Option<Hover>> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, position, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;
    let mut descriptions = BTreeSet::new();
    let mut range = None;

    for entry in entries {
        let Some(hover) = project.hover(&entry, offset, &options.remaining(started))? else {
            continue;
        };

        let current = position::range(&source.text, hover.location.range, encoding)?;

        if range.as_ref().is_some_and(|range| range != &current) {
            return Err(io::Error::other("contextual hover ranges disagree"));
        }

        range = Some(current);

        let mut description = code(&hover.text, markup);

        if hover.deprecated {
            description.push_str("\n\nDeprecated.");
        }

        if let Some(symbol) = &hover.documentation
            && let Some(documentation) =
                project.documentation_for(path, symbol, &options.remaining(started))?
        {
            if let Some(text) = documentation.documentation {
                description.push_str("\n\n");
                description.push_str(&text);
            }

            if let Some(sample) = documentation.code_sample {
                description.push_str("\n\n");
                description.push_str(&code(&sample, markup));
            }

            if let Some(link) = documentation.learn_more_link {
                description.push_str("\n\n");
                description.push_str(&link);
            }
        }

        descriptions.insert(description);
    }

    options.check(started)?;

    Ok(range.map(|range| Hover {
        contents: MarkupContent {
            kind: markup.clone(),
            value: descriptions.into_iter().collect::<Vec<_>>().join("\n\n"),
        }
        .into(),
        range: Some(range),
    }))
}
