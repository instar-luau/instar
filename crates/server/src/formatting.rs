use std::{io, path::Path, time::Instant};

use instar_analysis::{Completion, Options, error::invalid};
use instar_core::project::Project;
use instar_format::Formatter;
use tokio::time;

use tower_lsp_server::{
    gen_lsp_types::{
        DocumentFormattingParams, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams,
        Position, PositionEncodingKind, Range, TextEdit, Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    position,
    session::{Session, failure},
    workers::Output,
};

impl Session {
    pub(crate) async fn format(
        &self,
        parameters: DocumentFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.formatted(parameters.text_document.uri, None).await
    }

    pub(crate) async fn format_range(
        &self,
        parameters: DocumentRangeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        self.formatted(parameters.text_document.uri, Some(parameters.range))
            .await
    }

    pub(crate) async fn format_character(
        &self,
        parameters: DocumentOnTypeFormattingParams,
    ) -> Result<Option<Vec<TextEdit>>> {
        let start = match parameters.ch.as_str() {
            "\n" => Position {
                line: parameters.position.line.saturating_sub(1),
                character: 0,
            },

            ";" => parameters.position,
            _ => return Ok(None),
        };

        self.formatted(
            parameters.text_document.uri,
            Some(Range {
                start,
                end: parameters.position,
            }),
        )
        .await
    }

    async fn formatted(&self, uri: Uri, range: Option<Range>) -> Result<Option<Vec<TextEdit>>> {
        let (snapshot, path, encoding) = {
            let workspace = self.workspace.lock().expect("workspace lock");
            workspace.editable(&uri)?;

            workspace.capture(&uri)?
        };

        let options = Options {
            cancellation: self.options.cancellation.child(),
            ..self.options.clone()
        };

        let timeout = options.timeout;

        let (_guard, response) = self
            .workers
            .submit(
                snapshot.clone(),
                options,
                move |project, formatter, options, started| {
                    format(
                        &path, range, &encoding, project, formatter, options, started,
                    )
                },
            )
            .map_err(|error| failure(&error))?;

        let output = time::timeout(timeout, response)
            .await
            .map_err(|_| {
                failure(&io::Error::new(
                    io::ErrorKind::TimedOut,
                    "formatting deadline expired",
                ))
            })?
            .map_err(|_| failure(&io::Error::other("formatting worker stopped")))?;

        if !self
            .workspace
            .lock()
            .expect("workspace lock")
            .current(&snapshot, &output.inputs)
        {
            return Err(Error::content_modified());
        }

        output.result.map_err(|error| failure(&error))
    }
}

fn format(
    path: &Path,
    range: Option<Range>,
    encoding: &PositionEncodingKind,
    project: &mut Project,
    formatter: &mut Formatter,
    options: &Options,
    started: Instant,
) -> Output<Option<Vec<TextEdit>>> {
    let result = (|| {
        options.check(started)?;

        let Some(configuration) =
            project.formatting_configuration(path, &options.remaining(started))?
        else {
            return Ok(None);
        };

        let source = project.source(path)?;

        let result = if let Some(range) = range {
            let start = position::offset(&source.text, range.start, encoding).map_err(invalid)?;
            let end = position::offset(&source.text, range.end, encoding).map_err(invalid)?;

            formatter.format_range(
                source.text.as_bytes(),
                [start, end],
                &configuration,
                &options.remaining(started),
            )?
        } else {
            formatter.format(
                source.text.as_bytes(),
                &configuration,
                &options.remaining(started),
            )?
        };

        options.check(started)?;

        if let Completion::Incomplete(reason) = result.completion {
            return Err(reason.error());
        }

        if !result.diagnostics.is_empty() {
            return Err(invalid(
                result
                    .diagnostics
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("\n"),
            ));
        }

        let text = result
            .output
            .ok_or_else(|| io::Error::other("formatter returned no output"))?;

        let text = String::from_utf8(text).map_err(invalid)?;

        if source.text.as_ref() == text {
            return Ok(None);
        }

        let (start, suffix) = if range.is_some() {
            let start = source
                .text
                .split_inclusive('\n')
                .zip(text.split_inclusive('\n'))
                .take_while(|(left, right)| left == right)
                .map(|(line, _)| line.len())
                .sum::<usize>();

            let suffix = source.text[start..]
                .split_inclusive('\n')
                .rev()
                .zip(text[start..].split_inclusive('\n').rev())
                .take_while(|(left, right)| left == right)
                .map(|(line, _)| line.len())
                .sum::<usize>();

            (start, suffix)
        } else {
            (0, 0)
        };

        let edit = TextEdit {
            range: position::range(&source.text, [start, source.text.len() - suffix], encoding)?,
            new_text: text[start..text.len() - suffix].to_owned(),
        };

        options.check(started)?;

        Ok(Some(vec![edit]))
    })();

    let inputs = if result.is_ok() {
        project
            .configuration(path.parent().expect("absolute source parent"))
            .map_or_else(
                |_| project.watch_inputs(),
                |settings| {
                    let mut inputs = settings.inputs.clone();
                    inputs.insert(path.to_path_buf());

                    inputs
                },
            )
    } else {
        project.watch_inputs()
    };

    Output { result, inputs }
}
