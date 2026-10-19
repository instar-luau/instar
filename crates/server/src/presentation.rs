use std::{
    collections::BTreeMap,
    ops::{ControlFlow, Range},
};

use instar_bridge::{Frontend, Hint, HintKind, OccurrenceRole};
use instar_core::analysis::{self, FoldKind, TokenKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{
        Color, ColorInformation, ColorPresentation, ColorPresentationParams, DocumentColorParams,
        FoldingRange, FoldingRangeKind, FoldingRangeParams, InlayHint, InlayHintKind,
        InlayHintParams, Label, SelectionRange, SelectionRangeParams, SemanticToken,
        SemanticTokens, SemanticTokensDelta, SemanticTokensDeltaParams,
        SemanticTokensDeltaResponse, SemanticTokensEdit, SemanticTokensLegend,
        SemanticTokensParams, SemanticTokensRangeParams, TextEdit, Tooltip,
    },
    jsonrpc::{Error, ErrorCode, Result},
};

use super::{
    configuration,
    document::Document,
    project::{Project, failure},
};

const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "type",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "function",
    "method",
    "keyword",
    "comment",
    "string",
    "number",
    "operator",
    "decorator",
];

const TOKEN_MODIFIERS: &[&str] = &["declaration", "readonly", "modification"];

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Annotation {
    offset: usize,
    kind: String,
    label: String,
}

pub(super) fn legend(capabilities: &Value) -> SemanticTokensLegend {
    let supported = |pointer: &str, values: &[&str]| {
        let supported = capabilities.pointer(pointer).and_then(Value::as_array);

        values
            .iter()
            .filter(|value| {
                supported.is_some_and(|supported| {
                    supported
                        .iter()
                        .any(|candidate| candidate.as_str() == Some(**value))
                })
            })
            .map(|value| (*value).to_owned())
            .collect()
    };

    SemanticTokensLegend {
        token_types: supported("/textDocument/semanticTokens/tokenTypes", TOKEN_TYPES),
        token_modifiers: supported(
            "/textDocument/semanticTokens/tokenModifiers",
            TOKEN_MODIFIERS,
        ),
    }
}

pub(super) fn inlay_hint_source(
    project: &mut Project,
    parameters: &InlayHintParams,
) -> Result<ControlFlow<Option<Vec<InlayHint>>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    document.bytes(parameters.range, project.encoding)?;

    if [
        configuration::Hint::VariableTypes,
        configuration::Hint::ParameterNames,
        configuration::Hint::ReturnTypes,
    ]
    .into_iter()
    .any(|category| project.configuration.hint(category))
    {
        Ok(ControlFlow::Continue(()))
    } else {
        Ok(ControlFlow::Break(Some(Vec::new())))
    }
}

pub(super) fn inlay_hint(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let range = document.bytes(parameters.range, project.encoding)?;
    let mut hints = Vec::new();

    for hint in frontend.hints(&module).map_err(failure)? {
        if hint.location.module != module || !enabled(project, &hint) {
            continue;
        }

        let position = document
            .native_range(hint.location.range, project.encoding)?
            .start;

        let offset = document.offset(position, project.encoding)?;

        if offset < range.start || offset > range.end {
            continue;
        }

        let data = project.data(
            &module,
            Annotation {
                offset,
                kind: category(hint.kind)?.to_owned(),
                label: hint.label.clone(),
            },
        )?;

        let mut hint = annotation(project, &document, &hint)?;
        hint.data = Some(data);

        if !hints.iter().any(|previous: &InlayHint| {
            previous.position == hint.position && previous.label == hint.label
        }) {
            hints.push(hint);
        }
    }

    Ok(Some(hints))
}

pub(super) fn inlay_hint_resolve_source(
    project: &mut Project,
    parameters: &InlayHint,
) -> Result<ControlFlow<InlayHint>> {
    let record: Annotation = match project.resolve(parameters.data.clone()) {
        Ok((_, record)) => record,

        Err(error) if error.code == ErrorCode::ContentModified => {
            return Ok(ControlFlow::Continue(()));
        }

        Err(error) => return Err(error),
    };

    if !configured(project, &record.kind) {
        return Err(Error::content_modified());
    }

    Ok(ControlFlow::Continue(()))
}

pub(super) fn inlay_hint_resolve(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: InlayHint,
) -> Result<InlayHint> {
    let (module, record): (_, Annotation) = project.resolve(parameters.data)?;

    if !configured(project, &record.kind) {
        return Err(Error::content_modified());
    }

    let document = project.snapshot(&module)?;

    let hint = frontend
        .hints(&module)
        .map_err(failure)?
        .into_iter()
        .find(|hint| {
            hint.location.module == module
                && enabled(project, hint)
                && category(hint.kind).is_ok_and(|kind| kind == record.kind)
                && hint.label == record.label
                && document
                    .native_range(hint.location.range, project.encoding)
                    .and_then(|range| document.offset(range.start, project.encoding))
                    .is_ok_and(|offset| offset == record.offset)
        })
        .ok_or_else(Error::content_modified)?;

    let mut resolved = annotation(project, &document, &hint)?;

    let description = match hint.kind {
        HintKind::Type => "Inferred binding type",
        HintKind::Parameter => "Call parameter",
        HintKind::Return => "Inferred return type",
        _ => return Err(Error::invalid_params("cannot resolve an unknown hint kind")),
    };

    let mut details = vec![format!(
        "{description}: {}",
        hint.label.trim_start_matches(": ")
    )];

    let offset = if hint.kind == HintKind::Type {
        document
            .analysis
            .tokens
            .iter()
            .find(|token| token.range.end == record.offset)
            .map_or(record.offset, |token| token.range.start)
    } else {
        record.offset
    };

    let position = document.position(offset, project.encoding)?;
    let (line, column) = document.native(position, project.encoding)?;

    for information in frontend.hover(&module, line, column).map_err(failure)? {
        let detail = if information.symbol.is_empty() {
            information.type_name
        } else {
            format!("{}: {}", information.symbol, information.type_name)
        };

        if !detail.is_empty() && !details.contains(&detail) {
            details.push(detail);
        }

        if !information.documentation.is_empty() && !details.contains(&information.documentation) {
            details.push(information.documentation);
        }
    }

    resolved.tooltip = Some(Tooltip::String(details.join("\n\n")));
    resolved.data = Some(project.data(&module, record)?);

    Ok(resolved)
}

pub(super) fn semantic_tokens_full(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &SemanticTokensParams,
) -> Result<Option<SemanticTokens>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;

    let data = classify(
        project,
        frontend,
        &module,
        &document,
        0..document.text.len(),
    )?;

    let result_id = remember(project, &module, &data)?;

    Ok(Some(SemanticTokens {
        result_id: Some(result_id),
        data,
    }))
}

pub(super) fn semantic_tokens_full_delta(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &SemanticTokensDeltaParams,
) -> Result<Option<SemanticTokensDeltaResponse>> {
    let module = project.module(&parameters.text_document.uri)?;

    let previous = project
        .tokens
        .get(&module)
        .and_then(|results| {
            results
                .iter()
                .find(|(identifier, _)| identifier == &parameters.previous_result_id)
        })
        .map(|(_, data)| data.clone());

    let document = project.snapshot(&module)?;

    let data = classify(
        project,
        frontend,
        &module,
        &document,
        0..document.text.len(),
    )?;

    let result_id = Some(remember(project, &module, &data)?);

    let Some(previous) = previous else {
        return Ok(Some(SemanticTokensDeltaResponse::SemanticTokens(
            SemanticTokens { result_id, data },
        )));
    };

    let prefix = previous
        .iter()
        .zip(&data)
        .take_while(|(left, right)| left == right)
        .count();

    let suffix = previous[prefix..]
        .iter()
        .rev()
        .zip(data[prefix..].iter().rev())
        .take_while(|(left, right)| left == right)
        .count();

    let inserted = &data[prefix..data.len() - suffix];
    let removed = previous.len() - prefix - suffix;

    let edits = if removed == 0 && inserted.is_empty() {
        Vec::new()
    } else {
        vec![SemanticTokensEdit {
            start: u32::try_from(prefix.checked_mul(5).ok_or_else(Error::internal_error)?)
                .map_err(failure)?,
            delete_count: u32::try_from(removed.checked_mul(5).ok_or_else(Error::internal_error)?)
                .map_err(failure)?,
            data: (!inserted.is_empty()).then(|| {
                inserted
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
                    .collect()
            }),
        }]
    };

    Ok(Some(SemanticTokensDeltaResponse::SemanticTokensDelta(
        SemanticTokensDelta { result_id, edits },
    )))
}

pub(super) fn semantic_tokens_range(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &SemanticTokensRangeParams,
) -> Result<Option<SemanticTokens>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let range = document.bytes(parameters.range, project.encoding)?;
    let data = classify(project, frontend, &module, &document, range)?;

    Ok(Some(SemanticTokens {
        result_id: None,
        data,
    }))
}

pub(super) fn folding_range(
    project: &mut Project,
    parameters: &FoldingRangeParams,
) -> Result<Option<Vec<FoldingRange>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let lines = project.supports("/textDocument/foldingRange/lineFoldingOnly");

    let kinds = project
        .capabilities
        .pointer("/textDocument/foldingRange/foldingRangeKind/valueSet")
        .and_then(Value::as_array);

    let limit = project
        .capabilities
        .pointer("/textDocument/foldingRange/rangeLimit")
        .and_then(Value::as_u64)
        .and_then(|limit| usize::try_from(limit).ok())
        .unwrap_or(usize::MAX);

    let mut ranges = Vec::new();

    for fold in &document.analysis.folds {
        let range = document.range(fold.range.clone(), project.encoding)?;

        let end_line = if range.end.character == 0 {
            range.end.line.saturating_sub(1)
        } else {
            range.end.line
        };

        if range.start.line >= end_line {
            continue;
        }

        let (name, kind) = match fold.kind {
            FoldKind::Region => ("region", FoldingRangeKind::Region),
            FoldKind::Comment => ("comment", FoldingRangeKind::Comment),
            FoldKind::Imports => ("imports", FoldingRangeKind::Imports),
        };

        ranges.push(FoldingRange {
            start_line: range.start.line,
            start_character: (!lines).then_some(range.start.character),
            end_line,
            end_character: (!lines && range.end.character != 0).then_some(range.end.character),
            kind: kinds
                .is_none_or(|kinds| kinds.iter().any(|kind| kind.as_str() == Some(name)))
                .then_some(kind),
            collapsed_text: None,
        });
    }

    ranges.dedup();
    ranges.truncate(limit);

    Ok(Some(ranges))
}

pub(super) fn selection_range(
    project: &mut Project,
    parameters: SelectionRangeParams,
) -> Result<Option<Vec<SelectionRange>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let mut selections = Vec::new();

    for position in parameters.positions {
        let offset = document.offset(position, project.encoding)?;
        let mut parent: Option<Box<SelectionRange>> = None;

        for range in document.analysis.selection(offset).into_iter().rev() {
            let range = document.range(range, project.encoding)?;

            if parent
                .as_deref()
                .is_some_and(|parent| parent.range == range)
            {
                continue;
            }

            parent = Some(Box::new(SelectionRange { range, parent }));
        }

        selections.push(
            parent
                .map(|selection| *selection)
                .unwrap_or(SelectionRange {
                    range: document.range(offset..offset, project.encoding)?,
                    parent: None,
                }),
        );
    }

    Ok(Some(selections))
}

pub(super) fn document_color(
    project: &mut Project,
    parameters: &DocumentColorParams,
) -> Result<Vec<ColorInformation>> {
    let module = project.module(&parameters.text_document.uri)?;

    if !project.workspace.roblox(&module).map_err(failure)? {
        return Ok(Vec::new());
    }

    let document = project.snapshot(&module)?;

    document
        .analysis
        .colors
        .iter()
        .map(|color| {
            let [red, green, blue, alpha] = color.components;

            Ok(ColorInformation {
                range: document.range(color.range.clone(), project.encoding)?,
                color: Color {
                    red: red.to_string().parse().map_err(failure)?,
                    green: green.to_string().parse().map_err(failure)?,
                    blue: blue.to_string().parse().map_err(failure)?,
                    alpha: alpha.to_string().parse().map_err(failure)?,
                },
            })
        })
        .collect()
}

pub(super) fn color_presentation(
    project: &mut Project,
    parameters: &ColorPresentationParams,
) -> Result<Vec<ColorPresentation>> {
    let module = project.module(&parameters.text_document.uri)?;

    if !project.workspace.roblox(&module).map_err(failure)? {
        return Ok(Vec::new());
    }

    let document = project.snapshot(&module)?;
    let range = document.bytes(parameters.range, project.encoding)?;
    let color = parameters.color;

    analysis::color_presentations(
        &document.text,
        range,
        [color.red, color.green, color.blue, color.alpha].map(f64::from),
    )
    .into_iter()
    .map(|edit| {
        Ok(ColorPresentation {
            label: edit.replacement.clone(),
            text_edit: Some(TextEdit {
                range: document.range(edit.range, project.encoding)?,
                new_text: edit.replacement,
            }),
            additional_text_edits: None,
        })
    })
    .collect()
}

fn configured(project: &Project, kind: &str) -> bool {
    let category = match kind {
        "type" => configuration::Hint::VariableTypes,
        "parameter" => configuration::Hint::ParameterNames,
        "return" => configuration::Hint::ReturnTypes,
        _ => return false,
    };

    project.configuration.hint(category)
}

fn enabled(project: &Project, hint: &Hint) -> bool {
    let settings = &project.configuration;

    !hint.label.is_empty()
        && (!settings.hint(configuration::Hint::HideObvious) || !hint.obvious)
        && match hint.kind {
            HintKind::Type => settings.hint(configuration::Hint::VariableTypes),
            HintKind::Parameter => settings.hint(configuration::Hint::ParameterNames),
            HintKind::Return => settings.hint(configuration::Hint::ReturnTypes),
            _ => false,
        }
}

fn category(kind: HintKind) -> Result<&'static str> {
    match kind {
        HintKind::Type => Ok("type"),
        HintKind::Parameter => Ok("parameter"),
        HintKind::Return => Ok("return"),

        _ => Err(Error::invalid_params(
            "cannot classify an unknown hint kind",
        )),
    }
}

fn annotation(project: &Project, document: &Document, hint: &Hint) -> Result<InlayHint> {
    Ok(InlayHint {
        position: document
            .native_range(hint.location.range, project.encoding)?
            .start,
        label: Label::String(hint.label.clone()),
        kind: Some(if hint.kind == HintKind::Parameter {
            InlayHintKind::Parameter
        } else {
            InlayHintKind::Type
        }),
        text_edits: None,
        tooltip: None,
        padding_left: Some(hint.kind != HintKind::Parameter),
        padding_right: Some(hint.kind == HintKind::Parameter),
        data: None,
    })
}

fn classify(
    project: &Project,
    frontend: &mut Frontend<'_>,
    module: &str,
    document: &Document,
    requested: Range<usize>,
) -> Result<Vec<SemanticToken>> {
    let legend = legend(&project.capabilities);

    if legend.token_types.is_empty() {
        return Ok(Vec::new());
    }

    let mut roles = BTreeMap::new();

    for occurrence in frontend.occurrences(module).map_err(failure)? {
        if occurrence.location.module != module {
            continue;
        }

        let range = document.native_range(occurrence.location.range, project.encoding)?;
        let bytes = document.bytes(range, project.encoding)?;

        let role = roles
            .entry((bytes.start, bytes.end))
            .or_insert((false, false, false));

        role.0 |= occurrence.type_namespace;
        role.1 |= occurrence.role == OccurrenceRole::Declaration;

        role.2 |= matches!(
            occurrence.role,
            OccurrenceRole::Write | OccurrenceRole::ReadWrite
        );
    }

    let mut data = Vec::new();
    let mut previous = (0, 0);

    for token in &document.analysis.tokens {
        let mut kind = token.kind;
        let mut declaration = token.declaration;
        let mut modification = token.access.write() && !declaration;

        if let Some(&(types, declares, writes)) = roles.get(&(token.range.start, token.range.end)) {
            if types && matches!(kind, TokenKind::Variable | TokenKind::Property) {
                kind = TokenKind::Type;
            }

            declaration |= declares;
            modification |= writes && !declaration;
        }

        let name = token_name(kind);

        let fallback = match kind {
            TokenKind::TypeParameter => "type",
            TokenKind::Method => "function",
            TokenKind::Parameter | TokenKind::Property | TokenKind::Namespace => "variable",
            _ => name,
        };

        let Some(token_type) = legend
            .token_types
            .iter()
            .position(|supported| supported == name)
            .or_else(|| {
                legend
                    .token_types
                    .iter()
                    .position(|supported| supported == fallback)
            })
        else {
            continue;
        };

        let mut modifiers = 0;

        for (name, present) in [
            ("declaration", declaration),
            ("readonly", token.readonly),
            ("modification", modification),
        ] {
            if present
                && let Some(index) = legend
                    .token_modifiers
                    .iter()
                    .position(|supported| supported == name)
            {
                modifiers |= 1 << index;
            }
        }

        let start = token.range.start.max(requested.start);
        let end = token.range.end.min(requested.end);

        if start >= end {
            continue;
        }

        let mut offset = start;

        for segment in document.text[start..end].split_inclusive('\n') {
            let text = segment.trim_end_matches(['\n', '\r']);

            if !text.is_empty() {
                let position = document.position(offset, project.encoding)?;
                let finish = document.position(offset + text.len(), project.encoding)?;

                let delta_line = position
                    .line
                    .checked_sub(previous.0)
                    .ok_or_else(Error::internal_error)?;

                let delta_start = if delta_line == 0 {
                    position
                        .character
                        .checked_sub(previous.1)
                        .ok_or_else(Error::internal_error)?
                } else {
                    position.character
                };

                data.push(SemanticToken {
                    delta_line,
                    delta_start,
                    length: finish.character - position.character,
                    token_type: u32::try_from(token_type).map_err(failure)?,
                    token_modifiers_bitset: modifiers,
                });

                previous = (position.line, position.character);
            }

            offset += segment.len();
        }
    }

    Ok(data)
}

fn token_name(kind: TokenKind) -> &'static str {
    match kind {
        TokenKind::Namespace => "namespace",
        TokenKind::Type => "type",
        TokenKind::TypeParameter => "typeParameter",
        TokenKind::Parameter => "parameter",
        TokenKind::Variable => "variable",
        TokenKind::Property => "property",
        TokenKind::Function => "function",
        TokenKind::Method => "method",
        TokenKind::Keyword => "keyword",
        TokenKind::Comment => "comment",
        TokenKind::String => "string",
        TokenKind::Number => "number",
        TokenKind::Operator => "operator",
        TokenKind::Decorator => "decorator",
    }
}

fn remember(project: &mut Project, module: &str, data: &[SemanticToken]) -> Result<String> {
    let sequence = project.advance()?;
    let identifier = format!("{}:{sequence}", project.generation);
    let results = project.tokens.entry(module.to_owned()).or_default();
    results.push_back((identifier.clone(), data.to_vec()));

    while results.len() > 4 {
        results.pop_front();
    }

    while project.tokens.len() > 32 {
        let Some(oldest) = project
            .tokens
            .keys()
            .find(|key| key.as_str() != module)
            .cloned()
        else {
            break;
        };

        project.tokens.remove(&oldest);
    }

    Ok(identifier)
}
