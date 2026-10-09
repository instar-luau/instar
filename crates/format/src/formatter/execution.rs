use std::{io, time::Instant};

use instar_analysis::{Completion, Options};

use super::{builder::Builder, renderer, selection::Selection};
use crate::{Configuration, Diagnostic, Result};

pub(crate) fn format(
    source: &[u8],
    range: Option<[usize; 2]>,
    configuration: &Configuration,
    options: &Options,
) -> io::Result<Result> {
    let started = Instant::now();

    if let Some(reason) = options.interrupted(started) {
        return Ok(Result::interrupted(reason));
    }

    configuration.validate()?;
    let tree = vermis::parse(source);

    if let Some(reason) = options.interrupted(started) {
        return Ok(Result::interrupted(reason));
    }

    if range.is_none() && !tree.diagnostics.is_empty() {
        return Ok(Result {
            output: None,
            diagnostics: tree
                .diagnostics
                .iter()
                .map(|diagnostic| Diagnostic {
                    range: [diagnostic.span.start, diagnostic.span.end],
                    message: diagnostic.message.to_owned(),
                })
                .collect(),
            completion: Completion::Complete,
        });
    }

    let selection = match range
        .map(|range| Selection::new(&tree, range, configuration, options, started))
        .transpose()
    {
        Ok(selection) => selection,

        Err(error) => {
            return options
                .interrupted(started)
                .map_or_else(|| Err(error), |reason| Ok(Result::interrupted(reason)));
        }
    };

    if let Some(None) = selection {
        return Ok(Result {
            output: Some(source.to_vec()),
            diagnostics: Vec::new(),
            completion: Completion::Complete,
        });
    }

    let selection = selection.flatten();

    let plan = match Builder::build(&tree, selection.as_ref(), configuration, options, started) {
        Ok(plan) => plan,
        Err(reason) => return Ok(Result::interrupted(reason)),
    };

    let mut result = renderer::emit(&tree, configuration, &plan, options, started);

    if let Some(selection) = selection
        && let Some(output) = result.output.as_mut()
    {
        let mut combined = source[..selection.range.start].to_vec();
        combined.append(output);
        combined.extend_from_slice(&source[selection.range.end..]);
        *output = combined;
    }

    if let Some(reason) = options.interrupted(started) {
        return Ok(Result::interrupted(reason));
    }

    Ok(result)
}
