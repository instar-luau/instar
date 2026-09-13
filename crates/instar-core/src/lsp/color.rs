use super::{Backend, Position, Result, protocol};
use tower_lsp_server::jsonrpc::Error;

pub(super) async fn colors(
    server: &Backend,
    parameters: protocol::DocumentColorParams,
) -> Result<Vec<protocol::ColorInformation>> {
    let entries = server
        .query(
            protocol::TextDocumentPositionParams {
                text_document: parameters.text_document,
                position: Position::default(),
            },
            "colors",
        )
        .await?;

    Ok(entries
        .into_iter()
        .filter_map(|entry| {
            let [red, green, blue, alpha] = entry.native.color?;

            Some(protocol::ColorInformation {
                range: entry.location?.range,
                color: protocol::Color {
                    red,
                    green,
                    blue,
                    alpha,
                },
            })
        })
        .collect())
}

pub(super) fn presentations(
    parameters: &protocol::ColorPresentationParams,
) -> Result<Vec<protocol::ColorPresentation>> {
    let color = &parameters.color;

    if [color.red, color.green, color.blue, color.alpha]
        .into_iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(&value))
    {
        return Err(Error::invalid_params(
            "color channels must be between zero and one",
        ));
    }

    let channels =
        [color.red, color.green, color.blue].map(|value| format!("{:.0}", value * 255.0));

    let bytes = channels
        .iter()
        .map(|channel| channel.parse::<u8>().map_err(super::internal_error))
        .collect::<Result<Vec<_>>>()?;

    let labels = [
        format!("Color3.new({}, {}, {})", color.red, color.green, color.blue),
        format!(
            "Color3.fromRGB({}, {}, {})",
            channels[0], channels[1], channels[2]
        ),
        format!(
            "Color3.fromHex(\"#{:02X}{:02X}{:02X}\")",
            bytes[0], bytes[1], bytes[2]
        ),
    ];

    Ok(labels
        .into_iter()
        .map(|label| protocol::ColorPresentation {
            text_edit: Some(protocol::TextEdit {
                range: parameters.range,
                new_text: label.clone(),
            }),
            label,
            additional_text_edits: None,
        })
        .collect())
}
