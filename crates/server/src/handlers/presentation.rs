use instar_project::{
    PresentationKind, PresentationOutput, PresentationQuery, Query, QueryOutput, QueryReport,
    Token, TokenKind, TokenModifier,
};

use instar_shared::{Location as DomainLocation, SourceVersion, Span};
use serde_json::json;

use tower_lsp_server::{
    gen_lsp_types::{
        Color, ColorInformation, ColorPresentation, ColorPresentationParams, DocumentColorParams,
        FoldingRange, FoldingRangeKind, FoldingRangeParams, Position, PositionEncodingKind,
        ProgressToken, Range, SelectionRange, SelectionRangeParams, SemanticToken, SemanticTokens,
        SemanticTokensDelta, SemanticTokensDeltaParams, SemanticTokensDeltaResponse,
        SemanticTokensEdit, SemanticTokensLegend, SemanticTokensParams, SemanticTokensRangeParams,
        Uri,
    },
    jsonrpc::{Error, Result},
};

use crate::{
    Server, position,
    session::{TokenRecord, failure},
};

pub(crate) async fn tokens(
    server: &Server,
    parameters: SemanticTokensParams,
) -> Result<Option<SemanticTokens>> {
    let (report, source, legend, data) = prepared(
        server,
        &parameters.text_document.uri,
        None,
        parameters.work_done_progress_params.work_done_token,
    )
    .await?;

    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    let result_id = if state.capabilities.token_deltas() {
        Some(state.identities.store_tokens(TokenRecord {
            source,
            legend,
            tokens: data.clone(),
        })?)
    } else {
        None
    };

    Ok(Some(SemanticTokens { result_id, data }))
}

pub(crate) async fn delta(
    server: &Server,
    parameters: SemanticTokensDeltaParams,
) -> Result<Option<SemanticTokensDeltaResponse>> {
    let (report, source, legend, data) = prepared(
        server,
        &parameters.text_document.uri,
        None,
        parameters.work_done_progress_params.work_done_token,
    )
    .await?;

    let mut state = server.state.lock().expect("session state");

    if !state.current(&server.project, &report.inputs) {
        return Err(Error::content_modified());
    }

    if !state.capabilities.token_deltas() {
        return Err(Error::invalid_params(
            "semantic token deltas were not negotiated",
        ));
    }

    let changes = state
        .identities
        .tokens
        .get(&parameters.previous_result_id)
        .filter(|record| record.source.identity == source.identity && record.legend == legend)
        .map(|record| difference(&record.tokens, &data))
        .transpose()?;

    let result_id = Some(state.identities.store_tokens(TokenRecord {
        source,
        legend,
        tokens: data.clone(),
    })?);

    Ok(Some(match changes {
        Some(edits) => SemanticTokensDelta { result_id, edits }.into(),
        None => SemanticTokens { result_id, data }.into(),
    }))
}

pub(crate) async fn token_range(
    server: &Server,
    parameters: SemanticTokensRangeParams,
) -> Result<Option<SemanticTokens>> {
    let supported = {
        let state = server.state.lock().expect("session state");

        state.capabilities.range_tokens()
    };

    if !supported {
        return Err(Error::invalid_params(
            "range semantic tokens were not negotiated",
        ));
    }

    let (_, _, _, data) = prepared(
        server,
        &parameters.text_document.uri,
        Some(parameters.range),
        parameters.work_done_progress_params.work_done_token,
    )
    .await?;

    Ok(Some(SemanticTokens {
        result_id: None,
        data,
    }))
}

pub(crate) async fn folds(
    server: &Server,
    parameters: FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;

    let report = server
        .query(
            snapshot,
            Query::Presentation(PresentationQuery {
                source: source.version,
                kind: PresentationKind::Folds,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Presentation(PresentationOutput::Folds(folds)) = report.output else {
        return Err(Error::internal_error());
    };

    let capabilities = server
        .state
        .lock()
        .expect("session state")
        .capabilities
        .client
        .text_document
        .as_ref()
        .and_then(|document| document.folding_range.as_ref())
        .cloned()
        .unwrap_or_default();

    let line_only = capabilities.line_folding_only.unwrap_or(false);

    let comments = capabilities
        .folding_range_kind
        .as_ref()
        .and_then(|kind| kind.value_set.as_ref())
        .is_none_or(|kinds| kinds.contains(&FoldingRangeKind::Comment));

    let limit = capabilities
        .range_limit
        .map(usize::try_from)
        .transpose()
        .map_err(|error| {
            Error::invalid_params(format!("fold range limit exceeds supported size: {error}"))
        })?
        .unwrap_or(usize::MAX);

    let mut result = Vec::new();

    for fold in folds {
        if result.len() == limit {
            break;
        }

        let range = server.range(
            &report.snapshot,
            DomainLocation {
                source: source.version,
                span: Span {
                    start: fold.range[0],
                    end: fold.range[1],
                },
            },
        )?;

        let mut end_line = range.end.line;

        if line_only
            && (range.end.character == 0
                || !source
                    .text
                    .get(fold.range[1]..)
                    .ok_or_else(Error::internal_error)?
                    .split(['\r', '\n'])
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .is_empty())
        {
            end_line = end_line.saturating_sub(1);
        }

        if range.start.line >= end_line
            || result
                .last()
                .is_some_and(|previous: &FoldingRange| previous.start_line == range.start.line)
        {
            continue;
        }

        result.push(FoldingRange {
            start_line: range.start.line,
            start_character: None,
            end_line,
            end_character: (!line_only).then_some(range.end.character),
            kind: (comments && fold.comment).then_some(FoldingRangeKind::Comment),
            collapsed_text: None,
        });
    }

    Ok(Some(result))
}

pub(crate) async fn selections(
    server: &Server,
    parameters: SelectionRangeParams,
) -> Result<Option<Vec<SelectionRange>>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;
    let encoding = server.state.lock().expect("session state").encoding.clone();

    let offsets = parameters
        .positions
        .into_iter()
        .map(|selected| position::offset(&source.text, selected, &encoding))
        .collect::<Result<Vec<_>>>()?;

    let count = offsets.len();

    let report = server
        .query(
            snapshot,
            Query::Presentation(PresentationQuery {
                source: source.version,
                kind: PresentationKind::Selections(offsets),
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Presentation(PresentationOutput::Selections(selections)) = report.output
    else {
        return Err(Error::internal_error());
    };

    if selections.len() != count {
        return Err(Error::internal_error());
    }

    let mut result = Vec::new();

    for spans in selections {
        let mut parent: Option<Box<SelectionRange>> = None;

        for span in spans.into_iter().rev() {
            span.validate(&source).map_err(|error| failure(&error))?;

            let range = position::range(&source.text, [span.start, span.end], &encoding)
                .map_err(|error| failure(&error))?;

            if let Some(parent) = &parent
                && (range.start < parent.range.start || range.end > parent.range.end)
            {
                return Err(Error::invalid_params("selection hierarchy is not nested"));
            }

            parent = Some(Box::new(SelectionRange { range, parent }));
        }

        result.push(*parent.ok_or_else(|| Error::invalid_params("selection hierarchy is empty"))?);
    }

    Ok(Some(result))
}

pub(crate) async fn colors(
    server: &Server,
    parameters: DocumentColorParams,
) -> Result<Vec<ColorInformation>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;

    let report = server
        .query(
            snapshot,
            Query::Presentation(PresentationQuery {
                source: source.version,
                kind: PresentationKind::Colors,
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Presentation(PresentationOutput::Colors(colors)) = report.output else {
        return Err(Error::internal_error());
    };

    colors
        .into_iter()
        .map(|color| {
            validate_color(color.rgba)?;

            Ok(ColorInformation {
                range: server.range(&report.snapshot, color.location)?,
                color: serde_json::from_value::<Color>(json!({
                    "red": color.rgba[0],
                    "green": color.rgba[1],
                    "blue": color.rgba[2],
                    "alpha": color.rgba[3],
                }))
                .map_err(|error| Error {
                    message: error.to_string().into(),
                    ..Error::internal_error()
                })?,
            })
        })
        .collect()
}

pub(crate) async fn color(
    server: &Server,
    parameters: ColorPresentationParams,
) -> Result<Vec<ColorPresentation>> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(&parameters.text_document.uri, &options)?;
    let encoding = server.state.lock().expect("session state").encoding.clone();

    let range = Span::new(
        position::offset(&source.text, parameters.range.start, &encoding)?,
        position::offset(&source.text, parameters.range.end, &encoding)?,
        &source,
    )
    .map_err(|error| failure(&error))?;

    let rgba = [
        parameters.color.red,
        parameters.color.green,
        parameters.color.blue,
        parameters.color.alpha,
    ]
    .map(f64::from);

    validate_color(rgba)?;

    let report = server
        .query(
            snapshot,
            Query::Presentation(PresentationQuery {
                source: source.version,
                kind: PresentationKind::Color { range, rgba },
            }),
            parameters.work_done_progress_params.work_done_token,
            &options,
        )
        .await?;

    let QueryOutput::Presentation(PresentationOutput::ColorRepresentations(colors)) = report.output
    else {
        return Err(Error::internal_error());
    };

    colors
        .into_iter()
        .map(|color| {
            Ok(ColorPresentation {
                label: color.label,
                text_edit: Some(server.text_edit(&report.snapshot, &color.edit)?),
                additional_text_edits: None,
            })
        })
        .collect()
}

async fn prepared(
    server: &Server,
    uri: &Uri,
    range: Option<Range>,
    token: Option<ProgressToken>,
) -> Result<(
    QueryReport,
    SourceVersion,
    SemanticTokensLegend,
    Vec<SemanticToken>,
)> {
    let options = server.options.child();
    let (snapshot, source) = server.capture(uri, &options)?;

    let (provider, encoding) = {
        let state = server.state.lock().expect("session state");

        if range.is_none() && !state.capabilities.full_tokens() {
            return Err(Error::invalid_params(
                "full semantic tokens were not negotiated",
            ));
        }

        (
            state
                .capabilities
                .tokens
                .clone()
                .ok_or_else(|| Error::invalid_params("semantic tokens were not negotiated"))?,
            state.encoding.clone(),
        )
    };

    let span = range
        .map(|range| {
            Span::new(
                position::offset(&source.text, range.start, &encoding)?,
                position::offset(&source.text, range.end, &encoding)?,
                &source,
            )
            .map_err(|error| failure(&error))
        })
        .transpose()?;

    let report = server
        .query(
            snapshot,
            Query::Presentation(PresentationQuery {
                source: source.version,
                kind: PresentationKind::Tokens { range: span },
            }),
            token,
            &options,
        )
        .await?;

    let QueryOutput::Presentation(PresentationOutput::Tokens { location, entries }) =
        &report.output
    else {
        return Err(Error::internal_error());
    };

    if location.source != source.version {
        return Err(Error::content_modified());
    }

    let selected = span.unwrap_or(Span {
        start: 0,
        end: source.text.len(),
    });

    let data = render(entries, &source.text, selected, &encoding, &provider.legend)?;

    Ok((report, source.version, provider.legend, data))
}

fn render(
    tokens: &[Token],
    source: &str,
    selected: Span,
    encoding: &PositionEncodingKind,
    legend: &SemanticTokensLegend,
) -> Result<Vec<SemanticToken>> {
    let mut result = Vec::new();
    let mut previous = Position::default();
    let mut ending = 0;

    for token in tokens {
        if token.range[0] < ending
            || token.range[0] > token.range[1]
            || source.get(token.range[0]..token.range[1]).is_none()
        {
            return Err(Error::invalid_params(
                "semantic tokens overlap or have invalid ranges",
            ));
        }

        ending = token.range[1];
        let mut start = token.range[0].max(selected.start);
        let end = token.range[1].min(selected.end);

        if start >= end {
            continue;
        }

        let name = token_name(token.kind);

        let Some(kind) = legend.token_types.iter().position(|kind| kind == name) else {
            continue;
        };

        let modifiers =
            legend
                .token_modifiers
                .iter()
                .enumerate()
                .try_fold(0_u32, |mask, (index, name)| {
                    let present = token
                        .modifiers
                        .iter()
                        .any(|modifier| modifier_name(*modifier) == name);

                    let bit = 1_u32
                        .checked_shl(u32::try_from(index).map_err(|error| Error {
                            message: error.to_string().into(),
                            ..Error::internal_error()
                        })?)
                        .ok_or_else(Error::internal_error)?;

                    Ok::<_, Error>(if present { mask | bit } else { mask })
                })?;

        while start < end {
            let text = &source[start..end];
            let length = text.find(['\r', '\n']).unwrap_or(text.len());

            if length != 0 {
                let current = position::end(&source[..start], encoding);
                let width = position::end(&text[..length], encoding).character;

                result.push(SemanticToken {
                    delta_line: current
                        .line
                        .checked_sub(previous.line)
                        .ok_or_else(Error::internal_error)?,
                    delta_start: if current.line == previous.line {
                        current
                            .character
                            .checked_sub(previous.character)
                            .ok_or_else(Error::internal_error)?
                    } else {
                        current.character
                    },
                    length: width,
                    token_type: u32::try_from(kind).map_err(|error| Error {
                        message: error.to_string().into(),
                        ..Error::internal_error()
                    })?,
                    token_modifiers_bitset: modifiers,
                });

                previous = current;
                start += length;
            }

            if start < end {
                start += if source[start..end].starts_with("\r\n") {
                    2
                } else {
                    1
                };
            }
        }
    }

    Ok(result)
}

fn difference(
    previous: &[SemanticToken],
    current: &[SemanticToken],
) -> Result<Vec<SemanticTokensEdit>> {
    let prefix = previous
        .iter()
        .zip(current)
        .take_while(|(left, right)| left == right)
        .count();

    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(current[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();

    if prefix == previous.len() && prefix == current.len() {
        return Ok(Vec::new());
    }

    let start = prefix
        .checked_mul(5)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(Error::internal_error)?;

    let delete_count = (previous.len() - prefix - suffix)
        .checked_mul(5)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(Error::internal_error)?;

    let data = current[prefix..current.len() - suffix]
        .iter()
        .flat_map(|token| {
            [
                token.delta_line,
                token.delta_start,
                token.length,
                token.token_type,
                token.token_modifiers_bitset,
            ]
        })
        .collect::<Vec<_>>();

    Ok(vec![SemanticTokensEdit {
        start,
        delete_count,
        data: (!data.is_empty()).then_some(data),
    }])
}

fn validate_color(rgba: [f64; 4]) -> Result<()> {
    if rgba
        .into_iter()
        .all(|channel| channel.is_finite() && (0.0..=1.0).contains(&channel))
    {
        Ok(())
    } else {
        Err(Error::invalid_params(
            "color components must be finite values between zero and one",
        ))
    }
}

fn token_name(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Comment => "comment",
        TokenKind::Keyword => "keyword",
        TokenKind::String => "string",
        TokenKind::Number => "number",
        TokenKind::Operator => "operator",
        TokenKind::Decorator => "decorator",
        TokenKind::Type => "type",
        TokenKind::TypeParameter => "typeParameter",
        TokenKind::Variable => "variable",
        TokenKind::Parameter => "parameter",
        TokenKind::Function => "function",
        TokenKind::Property => "property",
        TokenKind::Method => "method",
    }
}

fn modifier_name(modifier: TokenModifier) -> &'static str {
    match modifier {
        TokenModifier::Declaration => "declaration",
        TokenModifier::Modification => "modification",
        TokenModifier::DefaultLibrary => "defaultLibrary",
        TokenModifier::Deprecated => "deprecated",
    }
}
