use std::{io, path::Path, time::Instant};

use instar_analysis::Options;
use instar_bridge::frontend::Signature;
use instar_core::project::Project;

use tower_lsp_server::{
    gen_lsp_types::{
        ActiveParameter, ClientSignatureInformationOptions, Documentation, MarkupKind,
        ParameterInformation, ParameterInformationLabel, Position, PositionEncodingKind,
        SignatureHelp, SignatureHelpClientCapabilities, SignatureHelpParams, SignatureInformation,
    },
    jsonrpc::Result,
};

use crate::{markup, position, session::Session};

impl Session {
    pub(crate) async fn signatures(
        &self,
        parameters: SignatureHelpParams,
    ) -> Result<Option<SignatureHelp>> {
        let parameters = parameters.text_document_position_params;

        let capabilities = self
            .workspace
            .lock()
            .expect("workspace lock")
            .signature_capabilities
            .clone();

        self.query(
            &parameters.text_document.uri,
            &self.queries,
            move |path, encoding, project, options, started| {
                query(
                    path,
                    parameters.position,
                    encoding,
                    &capabilities,
                    project,
                    options,
                    started,
                )
            },
        )
        .await
    }
}

fn query(
    path: &Path,
    position: Position,
    encoding: &PositionEncodingKind,
    capabilities: &SignatureHelpClientCapabilities,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> io::Result<Option<SignatureHelp>> {
    options.check(started)?;
    let source = project.source(path)?;

    let offset = position::offset(&source.text, position, encoding)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error.to_string()))?;

    let entries = project.entries(&[path.to_path_buf()], &options.remaining(started))?;

    let [entry] = entries.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "signature help requires one unambiguous module context",
        ));
    };

    let Some(result) = project.signatures(entry, offset, &options.remaining(started))? else {
        return Ok(None);
    };

    let information = capabilities
        .signature_information
        .clone()
        .unwrap_or_default();

    let format = information
        .documentation_format
        .as_ref()
        .and_then(|formats| {
            formats
                .iter()
                .find(|format| matches!(format, MarkupKind::PlainText | MarkupKind::Markdown))
        })
        .cloned()
        .unwrap_or(MarkupKind::PlainText);

    let documentation = if let Some(symbol) = result.documentation {
        project
            .documentation_for(path, &symbol, &options.remaining(started))?
            .map(|entry| markup::documentation(entry, format))
    } else {
        None
    };

    let active_signature = result
        .active
        .map(u32::try_from)
        .transpose()
        .map_err(io::Error::other)?;

    let active_parameter = result
        .entries
        .get(result.active.unwrap_or(0))
        .map(|signature| active(signature, result.argument, &information))
        .transpose()?
        .flatten();

    let mut signatures = Vec::new();

    for signature in result.entries {
        options.check(started)?;

        signatures.push(present(
            signature,
            result.argument,
            &information,
            documentation.clone(),
        )?);
    }

    options.check(started)?;

    Ok(Some(SignatureHelp {
        signatures,
        active_signature,
        active_parameter,
    }))
}

fn present(
    signature: Signature,
    argument: usize,
    capabilities: &ClientSignatureInformationOptions,
    documentation: Option<Documentation>,
) -> io::Result<SignatureInformation> {
    let offsets = capabilities
        .parameter_information
        .as_ref()
        .and_then(|parameters| parameters.label_offset_support)
        == Some(true);

    let active_parameter = if capabilities.active_parameter_support == Some(true) {
        active(&signature, argument, capabilities)?
    } else {
        None
    };

    let mut parameters = Vec::new();

    for [start, end] in &signature.parameters {
        let label = if offsets {
            ParameterInformationLabel::Tuple((
                u32::try_from(signature.label[..*start].encode_utf16().count())
                    .map_err(io::Error::other)?,
                u32::try_from(signature.label[..*end].encode_utf16().count())
                    .map_err(io::Error::other)?,
            ))
        } else {
            ParameterInformationLabel::String(signature.label[*start..*end].to_owned())
        };

        parameters.push(ParameterInformation {
            label,
            documentation: None,
        });
    }

    Ok(SignatureInformation {
        label: signature.label,
        documentation,
        parameters: Some(parameters),
        active_parameter,
    })
}

fn active(
    signature: &Signature,
    argument: usize,
    capabilities: &ClientSignatureInformationOptions,
) -> io::Result<Option<ActiveParameter>> {
    let parameter = if argument < signature.parameters.len() {
        Some(argument)
    } else if signature.variadic {
        signature.parameters.len().checked_sub(1)
    } else {
        None
    };

    parameter.map_or_else(
        || {
            Ok((capabilities.no_active_parameter_support == Some(true))
                .then_some(ActiveParameter::Null))
        },
        |parameter| {
            Ok(Some(ActiveParameter::Int(
                u32::try_from(parameter).map_err(io::Error::other)?,
            )))
        },
    )
}
