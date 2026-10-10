use std::num::NonZeroUsize;

use instar_format::{FormatOutcome, FormatScope, settings::IndentStyle};

use instar_project::{
    FormatMode, FormatOverrides, FormatRequest, FormattingPatch, Input, Invocation,
};

use instar_shared::{Completion, Span};

use tower_lsp_server::{
    gen_lsp_types::{
        DocumentFormattingParams, DocumentOnTypeFormattingParams, DocumentRangeFormattingParams,
        DocumentRangesFormattingParams, FormattingOptions, ProgressToken, Range,
        TextEdit as ProtocolTextEdit, Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::{Server, position, session::failure};

pub(crate) async fn whole(
    server: &Server,
    parameters: DocumentFormattingParams,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    formatted(
        server,
        parameters.text_document.uri,
        FormatScope::Whole,
        parameters.options,
        parameters.work_done_progress_params.work_done_token,
    )
    .await
}

pub(crate) async fn range(
    server: &Server,
    parameters: DocumentRangeFormattingParams,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    let scope = ranges(
        server,
        &parameters.text_document.uri,
        vec![parameters.range],
    )?;

    formatted(
        server,
        parameters.text_document.uri,
        scope,
        parameters.options,
        parameters.work_done_progress_params.work_done_token,
    )
    .await
}

pub(crate) async fn multiple(
    server: &Server,
    parameters: DocumentRangesFormattingParams,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    if !server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .supports("/experimental/rangesFormatting")
    {
        return Err(Error::invalid_params(
            "multiple-range formatting was not negotiated",
        ));
    }

    let scope = ranges(server, &parameters.text_document.uri, parameters.ranges)?;

    formatted(
        server,
        parameters.text_document.uri,
        scope,
        parameters.options,
        parameters.work_done_progress_params.work_done_token,
    )
    .await
}

pub(crate) async fn character(
    server: &Server,
    parameters: DocumentOnTypeFormattingParams,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    let mut characters = parameters.ch.chars();

    let trigger = characters
        .next()
        .filter(|trigger| matches!(trigger, '\n' | ';'))
        .ok_or_else(|| Error::invalid_params("unsupported formatting trigger"))?;

    if characters.next().is_some() {
        return Err(Error::invalid_params(
            "formatting trigger must contain one character",
        ));
    }

    let (_, source) = server.capture(&parameters.text_document.uri)?;
    let encoding = server.state.lock().expect("session state").encoding.clone();
    let offset = position::offset(&source.text, parameters.position, &encoding)?;

    formatted(
        server,
        parameters.text_document.uri,
        FormatScope::OnType { offset, trigger },
        parameters.options,
        None,
    )
    .await
}

fn ranges(server: &Server, uri: &Uri, ranges: Vec<Range>) -> Result<FormatScope> {
    let (_, source) = server.capture(uri)?;
    let encoding = server.state.lock().expect("session state").encoding.clone();

    let ranges = ranges
        .into_iter()
        .map(|range| {
            Span::new(
                position::offset(&source.text, range.start, &encoding)?,
                position::offset(&source.text, range.end, &encoding)?,
                &source,
            )
            .map_err(|error| failure(&error))
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(FormatScope::Ranges(ranges))
}

async fn formatted(
    server: &Server,
    uri: Uri,
    scope: FormatScope,
    formatting: FormattingOptions,
    token: Option<ProgressToken>,
) -> Result<Option<Vec<ProtocolTextEdit>>> {
    let (_, source) = server.capture(&uri)?;
    server.state.lock().expect("session state").editable(&uri)?;

    let width = NonZeroUsize::new(
        usize::try_from(formatting.tab_size)
            .map_err(|_| Error::invalid_params("tab size exceeds supported width"))?,
    )
    .ok_or_else(|| Error::invalid_params("tab size must be positive"))?;

    let request = FormatRequest {
        invocation: Invocation {
            inputs: vec![Input::Source(source.version.identity)],
            ..Invocation::default()
        },
        scope,
        overrides: FormatOverrides {
            formatting: FormattingPatch {
                indent_style: Some(if formatting.insert_spaces {
                    IndentStyle::Spaces
                } else {
                    IndentStyle::Tabs
                }),
                indent_width: Some(width),
                final_newline: formatting.insert_final_newline,
                ..FormattingPatch::default()
            },
            ..FormatOverrides::default()
        },
        mode: FormatMode::Preview,
    };

    let options = server.options.child();
    let task = server.project.format(request, options.clone());

    let report = server
        .task(task, &options, token, "Formatting Luau document")
        .await?;

    if report.completion != Completion::Complete {
        return Err(failure(&report.completion.error()));
    }

    if !server
        .state
        .lock()
        .expect("session state")
        .current(&server.project, &report.inputs)
    {
        return Err(Error::content_modified());
    }

    if report.failed {
        return Err(Error::invalid_params(
            report
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }

    let mut edits = Vec::new();

    for result in report.results {
        if result.result.completion != Completion::Complete {
            return Err(failure(&result.result.completion.error()));
        }

        if let Some(proposal) = result.proposal {
            if !server
                .state
                .lock()
                .expect("session state")
                .current(&server.project, proposal.inputs())
            {
                return Err(Error::content_modified());
            }

            for edit in &proposal.proposal().edits {
                if edit.location.source != source.version {
                    return Err(Error::content_modified());
                }

                edits.push(server.text_edit(&report.snapshot, edit)?);
            }
        } else {
            match result.result.outcome {
                FormatOutcome::Unavailable { reason, .. } => return Err(failure(&reason.error())),

                FormatOutcome::Validated(proposal) if proposal.output() == source.text.as_ref() => {
                }

                FormatOutcome::Validated(_) => {
                    return Err(Error::invalid_params(
                        "formatting proposal did not complete project validation",
                    ));
                }
            }
        }
    }

    Ok(Some(edits))
}
