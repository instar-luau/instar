use std::collections::BTreeSet;

use instar_bridge::Frontend;

use tower_lsp_server::{
    gen_lsp_types::{Contents, Hover, HoverParams},
    jsonrpc::Result,
};

use crate::{documentation as resolution, error::failure, output::documentation, project::Project};

pub(crate) fn hover(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: HoverParams,
) -> Result<Option<Hover>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;

    let should_use_markdown =
        documentation::should_use_markdown(project, "/textDocument/hover/contentFormat");

    let mut sections = Vec::new();
    let mut range = None;
    let mut descriptions = BTreeSet::new();
    let information = frontend.hover(&module, line, column).map_err(failure)?;

    for value in information {
        let metadata = resolution::resolve(project, &module, &value.documentation)?;

        let text = documentation::render(
            project,
            &module,
            metadata.as_deref(),
            value.has_receiver,
            should_use_markdown,
        )?;

        let description = if descriptions.insert(text.clone()) {
            text.as_str()
        } else {
            ""
        };

        let section = if value.is_namespace {
            let name = if should_use_markdown {
                format!("**{}**", value.presentation)
            } else {
                value.presentation.clone()
            };

            if description.is_empty() {
                name
            } else {
                format!("{name}\n\n{description}")
            }
        } else {
            documentation::presentation(&value.presentation, description, should_use_markdown)?
        };

        if !sections.contains(&section) {
            sections.push(section);
        }

        if range.is_none() {
            range = Some(document.native_range(value.location.range, project.encoding)?);
        }
    }

    Ok((!sections.is_empty()).then(|| Hover {
        contents: Contents::MarkupContent(documentation::hover(&sections, should_use_markdown)),
        range,
    }))
}
