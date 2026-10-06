mod builder;
mod document;
mod quotes;
mod renderer;

use self::builder::Builder;
use crate::{Configuration, Diagnostic, Result};
use instar_analysis::{Completion, Options};
use std::{io, time::Instant};

/// Formats source within a shared analysis budget.
///
/// Syntax failures carry byte ranges. Interrupted operations never return partial output.
/// Parsing and individual emitter operations are budget-checked between calls.
///
/// # Errors
/// Returns invalid formatter configuration.
pub fn format(
    source: &[u8],
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

    if !tree.diagnostics.is_empty() {
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

    let plan = match Builder::build(&tree, configuration, options, started) {
        Ok(plan) => plan,
        Err(reason) => return Ok(Result::interrupted(reason)),
    };

    Ok(renderer::emit(
        &tree,
        configuration,
        &plan,
        options,
        started,
    ))
}
