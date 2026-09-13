use super::{Response, Result, protocol, state::State};

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Settings {
    #[serde(default)]
    pub inlay_hints: Options,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Options {
    #[serde(default)]
    variable_types: bool,

    #[serde(default)]
    parameter_types: bool,

    #[serde(default)]
    function_return_types: bool,

    #[serde(default)]
    parameter_names: Names,

    #[serde(default = "length")]
    type_hint_max_length: usize,
}

#[derive(Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Names {
    #[default]
    None,

    Literals,
    All,
}

fn length() -> usize {
    50
}

impl Default for Options {
    fn default() -> Self {
        serde_json::from_str("{}").expect("inlay hint defaults")
    }
}

pub(super) fn hints(
    state: &mut State,
    parameters: &protocol::InlayHintParams,
) -> Result<Vec<protocol::InlayHint>> {
    let options = &state.settings.inlay_hints;

    if !options.variable_types
        && !options.parameter_types
        && !options.function_return_types
        && matches!(options.parameter_names, Names::None)
    {
        return Ok(Vec::new());
    }

    let Response::Editor(entries) = state.query(
        &protocol::TextDocumentPositionParams {
            text_document: parameters.text_document.clone(),
            position: parameters.range.start,
        },
        "hints",
    )?
    else {
        return Err(super::internal_error("unexpected worker response"));
    };

    let options = &state.settings.inlay_hints;

    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let enabled = match entry.native.kind {
                Some(1) => options.variable_types,
                Some(2) => options.parameter_types,
                Some(3) => options.function_return_types,
                Some(4) => matches!(options.parameter_names, Names::All),
                Some(5) => !matches!(options.parameter_names, Names::None),
                _ => false,
            };

            let position = entry.location?.range.start;

            if !enabled || position < parameters.range.start || position > parameters.range.end {
                return None;
            }

            let description = entry.native.description?;
            let parameter = matches!(entry.native.kind, Some(4 | 5));

            let label = if parameter {
                format!("{description}:")
            } else {
                format!(": {description}")
            };

            let shortened = if label.chars().count() > options.type_hint_max_length {
                format!(
                    "{}…",
                    label
                        .chars()
                        .take(options.type_hint_max_length.saturating_sub(1))
                        .collect::<String>()
                )
            } else {
                label.clone()
            };

            Some(protocol::InlayHint {
                position,
                label: protocol::InlayHintLabel::String(shortened),
                kind: Some(if parameter {
                    protocol::InlayHintKind::PARAMETER
                } else {
                    protocol::InlayHintKind::TYPE
                }),
                tooltip: Some(protocol::InlayHintTooltip::MarkupContent(
                    protocol::MarkupContent {
                        kind: protocol::MarkupKind::Markdown,
                        value: format!("```luau\n{label}\n```"),
                    },
                )),
                padding_right: parameter.then_some(true),
                text_edits: None,
                padding_left: None,
                data: None,
            })
        })
        .collect())
}
