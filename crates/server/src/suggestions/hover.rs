use instar_bridge::Frontend;
use instar_core::presentation;

use tower_lsp_server::{
    gen_lsp_types::{Contents, Hover, HoverParams},
    jsonrpc::Result,
};

use crate::{error::failure, output::documentation, project::Project};

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

    for value in frontend.hover(&module, line, column).map_err(failure)? {
        let detail = presentation::hover(&value).map_err(failure)?;

        let documentation = documentation::documentation(
            project,
            &module,
            &value.documentation_symbol,
            &value.documentation,
            should_use_markdown,
        )?;

        let section = documentation::presentation(&detail, &documentation, should_use_markdown)?;

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
