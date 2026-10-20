use instar_bridge::{Hint, HintKind};
use instar_core::presentation::{self, Surface};

use tower_lsp_server::{
    gen_lsp_types::{InlayHint, InlayHintKind, Label, Position, Tooltip},
    jsonrpc::{Error, Result},
};

use crate::{error::failure, output::documentation, project::Project};

pub(crate) fn annotation(position: Position, hint: &Hint) -> Result<InlayHint> {
    let label = match hint.kind {
        HintKind::Type => {
            presentation::compact(&hint.snapshot, Surface::TypeHint).map_err(failure)?
        }

        HintKind::Return => {
            presentation::compact(&hint.snapshot, Surface::ReturnHint).map_err(failure)?
        }

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
        let sections =
            presentation::detail(&information.snapshot, &information.name).map_err(failure)?;

        let metadata = documentation::metadata(project, module, &information.documentation_symbol)?;

        let value = if let Some(index) = information.parameter_index {
            documentation::parameter(metadata.as_ref(), &information.name, index)
                .and_then(|entry| entry.get("documentation"))
        } else {
            metadata.as_ref()
        };

        let documentation = documentation::render(
            project,
            module,
            value,
            &information.documentation,
            should_use_markdown,
        )?;

        let detail = documentation::presentation(&sections, &documentation, should_use_markdown)?;

        if !details.contains(&detail) {
            details.push(detail);
        }
    }

    Ok(Tooltip::MarkupContent(documentation::markup(
        details.join("\n\n"),
        should_use_markdown,
    )))
}
