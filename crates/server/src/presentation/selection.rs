use tower_lsp_server::{
    gen_lsp_types::{SelectionRange, SelectionRangeParams},
    jsonrpc::Result,
};

use crate::project::Project;

pub(crate) fn selection_range(
    project: &mut Project,
    parameters: SelectionRangeParams,
) -> Result<Option<Vec<SelectionRange>>> {
    let module = project.module(&parameters.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let mut selections = Vec::new();

    for position in parameters.positions {
        let offset = document.offset(position, project.encoding)?;
        let mut parent: Option<Box<SelectionRange>> = None;

        for range in document.analysis().selection(offset).into_iter().rev() {
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
