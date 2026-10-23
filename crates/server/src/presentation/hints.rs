use std::ops::ControlFlow;

use instar_bridge::{Frontend, Hint, HintKind, HintSelector, Position};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use tower_lsp_server::{
    gen_lsp_types::{InlayHint, InlayHintParams},
    jsonrpc::{Error, Result},
};

use crate::{configuration, error::failure, output::hints, project::Project};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Annotation {
    kind: String,
    selectors: Vec<Selector>,
}

#[derive(Deserialize, Serialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
struct Selector {
    identity: String,
    line: u32,
    column: u32,
}

pub(crate) fn inlay_hint_source(
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
    .any(|category| project.configuration.is_hint_enabled(category))
    {
        Ok(ControlFlow::Continue(()))
    } else {
        Ok(ControlFlow::Break(Some(Vec::new())))
    }
}

pub(crate) fn inlay_hint(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: &InlayHintParams,
) -> Result<Option<Vec<InlayHint>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    document.bytes(parameters.range, project.encoding)?;

    let should_defer = project
        .capabilities
        .pointer("/textDocument/inlayHint/resolveSupport/properties")
        .and_then(Value::as_array)
        .is_some_and(|properties| properties.iter().any(|property| property == "tooltip"));

    let (start_line, start_column) = document.native(parameters.range.start, project.encoding)?;
    let (end_line, end_column) = document.native(parameters.range.end, project.encoding)?;
    let mut grouped: Vec<(InlayHint, Annotation, Vec<Hint>)> = Vec::new();

    for hint in frontend
        .hints_range(
            &module,
            Position {
                line: start_line,
                column: start_column,
            },
            Position {
                line: end_line,
                column: end_column,
            },
            !should_defer,
        )
        .map_err(failure)?
    {
        if hint.location.module != module || !is_enabled(project, &hint) {
            continue;
        }

        let position = document
            .native_range(hint.location.range, project.encoding)?
            .start;

        let annotation = hints::annotation(position, &hint)?;
        let kind = category(hint.kind)?.to_owned();

        let selector = Selector {
            identity: hint.location.identity.clone(),
            line: hint.location.range.start.line,
            column: hint.location.range.start.column,
        };

        if let Some((_, record, information)) = grouped.iter_mut().find(|(previous, record, _)| {
            previous.position == annotation.position
                && previous.label == annotation.label
                && record.kind == kind
        }) {
            if !record.selectors.contains(&selector) {
                record.selectors.push(selector);
            }

            information.push(hint);
        } else {
            grouped.push((
                annotation,
                Annotation {
                    kind,
                    selectors: vec![selector],
                },
                vec![hint],
            ));
        }
    }

    let mut hints = Vec::with_capacity(grouped.len());

    for (mut hint, record, information) in grouped {
        if !should_defer {
            hint.tooltip = Some(hints::tooltip(project, &module, &information)?);
        }

        hint.data = Some(project.data(&module, record)?);
        hints.push(hint);
    }

    Ok(Some(hints))
}

pub(crate) fn inlay_hint_resolve_source(
    project: &mut Project,
    parameters: &InlayHint,
) -> Result<ControlFlow<InlayHint>> {
    let Some((_, record)) = project.resolve::<Annotation>(parameters.data.clone())? else {
        return Ok(ControlFlow::Break(parameters.clone()));
    };

    if !is_configured(project, &record.kind) {
        return Err(Error::content_modified());
    }

    Ok(ControlFlow::Continue(()))
}

pub(crate) fn inlay_hint_resolve(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    mut parameters: InlayHint,
) -> Result<InlayHint> {
    let Some((module, record)) = project.resolve::<Annotation>(parameters.data.clone())? else {
        return Ok(parameters);
    };

    if !is_configured(project, &record.kind) {
        return Err(Error::content_modified());
    }

    let information = subjects(project, frontend, &module, &record)?;
    parameters.tooltip = Some(hints::tooltip(project, &module, &information)?);

    parameters.data = Some(project.data(&module, record)?);

    Ok(parameters)
}

fn subjects(
    project: &Project,
    frontend: &mut Frontend<'_>,
    module: &str,
    record: &Annotation,
) -> Result<Vec<Hint>> {
    let kind = match record.kind.as_str() {
        "type" => HintKind::Type,
        "parameter" => HintKind::Parameter,
        "return" => HintKind::Return,
        _ => return Err(Error::content_modified()),
    };

    let selectors: Vec<_> = record
        .selectors
        .iter()
        .map(|selector| HintSelector {
            identity: selector.identity.clone(),
            position: Position {
                line: selector.line,
                column: selector.column,
            },
            kind,
        })
        .collect();

    let information = frontend.hint_subjects(&selectors).map_err(failure)?;

    if information.is_empty()
        || information
            .iter()
            .any(|hint| hint.location.module != module || !is_enabled(project, hint))
        || selectors.iter().any(|selector| {
            !information.iter().any(|hint| {
                hint.location.identity == selector.identity
                    && hint.location.range.start == selector.position
                    && hint.kind == selector.kind
            })
        })
    {
        return Err(Error::content_modified());
    }

    Ok(information)
}

fn is_configured(project: &Project, kind: &str) -> bool {
    let category = match kind {
        "type" => configuration::Hint::VariableTypes,
        "parameter" => configuration::Hint::ParameterNames,
        "return" => configuration::Hint::ReturnTypes,
        _ => return false,
    };

    project.configuration.is_hint_enabled(category)
}

fn is_enabled(project: &Project, hint: &Hint) -> bool {
    let settings = &project.configuration;

    (!settings.is_hint_enabled(configuration::Hint::HideObvious) || !hint.obvious)
        && match hint.kind {
            HintKind::Type => settings.is_hint_enabled(configuration::Hint::VariableTypes),
            HintKind::Parameter => settings.is_hint_enabled(configuration::Hint::ParameterNames),
            HintKind::Return => settings.is_hint_enabled(configuration::Hint::ReturnTypes),
            _ => false,
        }
}

fn category(kind: HintKind) -> Result<&'static str> {
    match kind {
        HintKind::Type => Ok("type"),
        HintKind::Parameter => Ok("parameter"),
        HintKind::Return => Ok("return"),

        _ => Err(Error::invalid_params(
            "Unknown inlay hint kind cannot be classified.",
        )),
    }
}
