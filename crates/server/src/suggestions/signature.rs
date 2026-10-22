use std::ops::Range;

use instar_bridge::Frontend;

use tower_lsp_server::{
    gen_lsp_types::{
        ActiveParameter, Documentation, ParameterInformation, ParameterInformationLabel,
        SignatureHelp, SignatureHelpParams, SignatureInformation,
    },
    jsonrpc::Result,
};

use crate::{
    error::{failure, reason},
    output::documentation,
    project::Project,
};

pub(crate) fn signature_help(
    project: &mut Project,
    frontend: &mut Frontend<'_>,
    parameters: SignatureHelpParams,
) -> Result<Option<SignatureHelp>> {
    let position = parameters.text_document_position_params;
    let module = project.module(&position.text_document.uri)?;
    let document = project.snapshot(&module)?;
    let (line, column) = document.native(position.position, project.encoding)?;

    let should_use_markdown = documentation::should_use_markdown(
        project,
        "/textDocument/signatureHelp/signatureInformation/documentationFormat",
    );

    let mut signatures = Vec::new();
    let mut active_signature = None;
    let mut active_parameter = None;

    let should_use_offsets = project.has_capability(
        "/textDocument/signatureHelp/signatureInformation/parameterInformation/labelOffsetSupport",
    );

    for signature in frontend
        .signatures(&module, line, column)
        .map_err(failure)?
    {
        let active = signature
            .active_parameter
            .min(u32::try_from(signature.parameters.len().saturating_sub(1)).map_err(failure)?);

        let metadata = documentation::overload(
            project,
            &module,
            &signature.documentation_symbol,
            &signature.documentation_signature,
        )?;

        let documentation = documentation::render(
            project,
            &module,
            metadata.as_deref(),
            signature
                .parameters
                .first()
                .is_some_and(|parameter| parameter.source_index > 0),
            should_use_markdown,
        )?;

        let detail = if signature.expanded == signature.label {
            ""
        } else {
            &signature.expanded
        };

        let text = documentation::presentation(detail, &documentation, should_use_markdown)?;

        let parameters = signature
            .parameters
            .iter()
            .map(|parameter| {
                let label = parameter_label(
                    &signature.label,
                    parameter.range.clone(),
                    should_use_offsets,
                )?;

                let entry = documentation::parameter(
                    metadata.as_deref(),
                    &parameter.name,
                    parameter.source_index,
                );

                let text =
                    documentation::render(project, &module, entry, false, should_use_markdown)?;

                Ok(ParameterInformation {
                    label,
                    documentation: (!text.is_empty()).then(|| {
                        Documentation::MarkupContent(documentation::markup(
                            text,
                            should_use_markdown,
                        ))
                    }),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let value = SignatureInformation {
            label: signature.label,
            documentation: (!text.is_empty()).then(|| {
                Documentation::MarkupContent(documentation::markup(text, should_use_markdown))
            }),
            parameters: Some(parameters),
            active_parameter: project
                .has_capability(
                    "/textDocument/signatureHelp/signatureInformation/activeParameterSupport",
                )
                .then_some(ActiveParameter::Int(active)),
        };

        if signature.selected || active_signature.is_none() {
            active_signature = Some(u32::try_from(signatures.len()).map_err(failure)?);
            active_parameter = Some(ActiveParameter::Int(active));
        }

        signatures.push(value);
    }

    Ok((!signatures.is_empty()).then_some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter,
    }))
}

fn parameter_label(
    signature: &str,
    span: Range<usize>,
    should_use_offsets: bool,
) -> Result<ParameterInformationLabel> {
    let label = signature
        .get(span.clone())
        .ok_or_else(|| reason("Signature parameter span is outside the final label."))?;

    if should_use_offsets {
        return Ok(ParameterInformationLabel::Tuple((
            u32::try_from(signature[..span.start].encode_utf16().count()).map_err(failure)?,
            u32::try_from(signature[..span.end].encode_utf16().count()).map_err(failure)?,
        )));
    }

    Ok(ParameterInformationLabel::String(label.to_owned()))
}
