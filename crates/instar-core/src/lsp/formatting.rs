use super::{Response, Result, internal_error, path, protocol, state::State};

use crate::{
    project::Configuration,
    source::{PositionEncoding, Source},
};

use line_index::LineCol;

pub(super) fn offsets(source: &Source, range: protocol::Range) -> Result<std::ops::Range<usize>> {
    let offset = |position: protocol::Position| {
        source
            .offset(
                LineCol {
                    line: position.line,
                    col: position.character,
                },
                PositionEncoding::Utf16,
            )
            .map(usize::from)
            .map_err(|error| tower_lsp_server::jsonrpc::Error::invalid_params(error.to_string()))
    };

    let start = offset(range.start)?;
    let end = offset(range.end)?;

    if start > end {
        return Err(tower_lsp_server::jsonrpc::Error::invalid_params(
            "reversed range",
        ));
    }

    Ok(start..end)
}

fn fragment(configuration: &Configuration, text: &str) -> Result<String> {
    if let Ok(output) = configuration.format(text.as_bytes()) {
        return String::from_utf8(output).map_err(internal_error);
    }

    let wrapped = format!("return {text}");

    let output = configuration
        .format(wrapped.as_bytes())
        .map_err(internal_error)?;

    let tree = vermis::parse(output.as_slice().into());

    let block = tree
        .root_view()
        .and_then(|root| root.children().next())
        .ok_or_else(|| internal_error("missing formatted expression"))?;

    if block.children().count() != 1 {
        return Err(internal_error("selection contains statements"));
    }

    let Some(vermis::Parts::Return { values }) =
        block.children().next().and_then(vermis::View::parts)
    else {
        return Err(internal_error("selection is not an expression"));
    };

    if values.clone().count() != 1 {
        return Err(internal_error("selection contains multiple expressions"));
    }

    let expression = values
        .clone()
        .next()
        .ok_or_else(|| internal_error("missing expression"))?;

    String::from_utf8(output[expression.span().start..expression.span().end].to_vec())
        .map_err(internal_error)
}

pub(super) fn range(
    state: &mut State,
    parameters: &protocol::DocumentRangeFormattingParams,
) -> Result<Response> {
    let source = state
        .sources
        .read(&path(&parameters.text_document.uri)?)
        .map_err(internal_error)?;

    let text = source.text().map_err(internal_error)?;
    let selected = offsets(&source, parameters.range)?;

    if selected.is_empty() {
        return Ok(Response::Edits(None));
    }

    let configuration = Configuration::discover(source.path(), None).map_err(internal_error)?;

    if !configuration.options.enabled {
        return Ok(Response::Edits(None));
    }

    let formatted = fragment(&configuration, &text[selected.clone()])?;

    if formatted == text[selected.clone()] {
        return Ok(Response::Edits(None));
    }

    let ending = if text[selected.clone()].ends_with('\n') {
        if text.contains("\r\n") { "\r\n" } else { "\n" }
    } else {
        ""
    };

    let start = text[..selected.start]
        .rfind('\n')
        .map_or(0, |offset| offset + 1);

    let indentation = text[start..]
        .chars()
        .take_while(|character| matches!(character, ' ' | '\t'))
        .collect::<String>();

    let formatted = formatted
        .trim_end_matches(['\r', '\n'])
        .lines()
        .collect::<Vec<_>>()
        .join(&format!(
            "{ending_separator}{indentation}",
            ending_separator = if text.contains("\r\n") { "\r\n" } else { "\n" }
        ));

    let leading = indentation
        .get(selected.start - start..)
        .unwrap_or_default();

    let replacement = format!("{leading}{formatted}{ending}");
    let mut output = text.to_owned();
    output.replace_range(selected.clone(), &replacement);
    crate::format::validate(source.bytes(), output.as_bytes()).map_err(internal_error)?;

    Ok(Response::Edits((replacement != text[selected]).then(
        || {
            vec![protocol::TextEdit {
                range: parameters.range,
                new_text: replacement,
            }]
        },
    )))
}
