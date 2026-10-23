use std::{collections::BTreeMap, ops::Range};

use instar_bridge::{Frontend, OccurrenceRole};
use instar_core::analysis::TokenKind;
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{
        SemanticToken, SemanticTokens, SemanticTokensDelta, SemanticTokensDeltaParams,
        SemanticTokensDeltaResponse, SemanticTokensEdit, SemanticTokensLegend,
        SemanticTokensParams, SemanticTokensRangeParams,
    },
    jsonrpc::{Error, Result},
};

use crate::{document::Document, error::failure, project::Project};

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

pub(crate) fn legend(capabilities: &Value) -> SemanticTokensLegend {
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

pub(crate) fn semantic_tokens_full(
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

pub(crate) fn semantic_tokens_full_delta(
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

pub(crate) fn semantic_tokens_range(
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

    for occurrence in frontend.occurrences(module, &[]).map_err(failure)? {
        if occurrence.location.module != module {
            continue;
        }

        let bytes = document.native_bytes(occurrence.location.range)?;

        let (is_type, is_declared, is_written) = roles
            .entry((bytes.start, bytes.end))
            .or_insert((false, false, false));

        *is_type |= occurrence.type_namespace;
        *is_declared |= occurrence.role == OccurrenceRole::Declaration;

        *is_written |= matches!(
            occurrence.role,
            OccurrenceRole::Write | OccurrenceRole::ReadWrite
        );
    }

    let mut data = Vec::new();
    let mut previous = (0, 0);

    for token in &document.analysis().tokens {
        let mut kind = token.kind;
        let mut is_declaration = token.is_declaration;
        let mut is_modification = token.access.is_write() && !is_declaration;

        if let Some((is_type, is_declared, is_written)) =
            roles.get(&(token.range.start, token.range.end))
        {
            if *is_type && matches!(kind, TokenKind::Variable | TokenKind::Property) {
                kind = TokenKind::Type;
            }

            is_declaration |= *is_declared;
            is_modification |= *is_written && !is_declaration;
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

        for (name, is_present) in [
            ("declaration", is_declaration),
            ("readonly", token.is_read_only),
            ("modification", is_modification),
        ] {
            if is_present
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
