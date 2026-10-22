use instar_bridge::{Hint, HintKind};

use tower_lsp_server::{
    gen_lsp_types::{InlayHint, InlayHintKind, Label, Position, Tooltip},
    jsonrpc::{Error, Result},
};

use crate::{output::documentation, project::Project};

pub(crate) fn annotation(position: Position, hint: &Hint) -> Result<InlayHint> {
    let label = match hint.kind {
        HintKind::Type | HintKind::Return => format!(": {}", hint.presentation.compact),

        HintKind::Parameter => format!("{}:", hint.name),

        _ => {
            return Err(Error::invalid_params(
                "Unknown inlay hint kind cannot be displayed.",
            ));
        }
    };

    Ok(InlayHint {
        position,
        label: Label::String(label),
        kind: Some(if hint.kind == HintKind::Parameter {
            InlayHintKind::Parameter
        } else {
            InlayHintKind::Type
        }),
        text_edits: None,
        tooltip: None,
        padding_left: Some(false),
        padding_right: Some(hint.kind == HintKind::Parameter),
        data: None,
    })
}

pub(crate) fn tooltip(project: &Project, module: &str, information: &[Hint]) -> Result<Tooltip> {
    let mut details = Vec::new();

    let should_use_markdown =
        documentation::should_use_markdown(project, "/textDocument/hover/contentFormat");

    for information in information {
        let code = if information.name.is_empty() {
            information.presentation.expanded.clone()
        } else {
            format!(
                "{}: {}",
                information.name, information.presentation.expanded
            )
        };

        let metadata = documentation::overload(
            project,
            module,
            &information.documentation_symbol,
            &information.documentation_signature,
        )?;

        let value = if let Some(index) = information.parameter_index {
            documentation::parameter(metadata.as_deref(), &information.name, index)
        } else {
            metadata.as_deref()
        };

        let documentation =
            documentation::render(project, module, value, false, should_use_markdown)?;

        let detail = documentation::presentation(&code, &documentation, should_use_markdown)?;

        if !details.contains(&detail) {
            details.push(detail);
        }
    }

    Ok(Tooltip::MarkupContent(documentation::markup(
        details.join("\n\n"),
        should_use_markdown,
    )))
}
