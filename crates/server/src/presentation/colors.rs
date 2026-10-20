use instar_core::analysis;

use tower_lsp_server::{
    gen_lsp_types::{
        Color, ColorInformation, ColorPresentation, ColorPresentationParams, DocumentColorParams,
        TextEdit,
    },
    jsonrpc::Result,
};

use crate::{error::failure, project::Project};

pub(crate) fn document_color(
    project: &mut Project,
    parameters: &DocumentColorParams,
) -> Result<Vec<ColorInformation>> {
    let module = project.module(&parameters.text_document.uri)?;

    if !project.workspace.is_roblox(&module).map_err(failure)? {
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

pub(crate) fn color_presentation(
    project: &mut Project,
    parameters: &ColorPresentationParams,
) -> Result<Vec<ColorPresentation>> {
    let module = project.module(&parameters.text_document.uri)?;

    if !project.workspace.is_roblox(&module).map_err(failure)? {
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
