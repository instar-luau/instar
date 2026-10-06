use std::{
    io,
    time::{Duration, Instant},
};

use instar_analysis::{
    Options,
    process::{Outcome, Process},
};

use serde::{Deserialize, Serialize};

use crate::{Configuration, Result};

#[derive(Deserialize, Serialize)]
pub(crate) struct Request {
    pub(crate) source: Vec<u8>,
    pub(crate) configuration: Configuration,
    pub(crate) timeout: Duration,
}

#[derive(Deserialize, Serialize)]
pub(crate) enum Response {
    Formatted(Result),
    InvalidConfiguration(String),
}

/// Formats source in an isolated, cancellable worker.
///
/// Interrupted requests never return partial output.
///
/// # Errors
/// Returns invalid configuration, unavailable worker, or transport failures.
pub fn format(
    source: &[u8],
    configuration: &Configuration,
    options: &Options,
) -> io::Result<Result> {
    let started = Instant::now();

    if let Some(reason) = options.interrupted(started) {
        return Ok(Result::interrupted(reason));
    }

    let mut process = Process::start("formatter")?;

    let request = Request {
        source: source.to_vec(),
        configuration: configuration.clone(),
        timeout: options.timeout.saturating_sub(started.elapsed()),
    };

    match process.request(request, options, started)? {
        Outcome::Response(Response::Formatted(result)) => Ok(result),

        Outcome::Response(Response::InvalidConfiguration(message)) => {
            Err(instar_analysis::error::invalid(message))
        }

        Outcome::Interrupted(reason) => Ok(Result::interrupted(reason)),
    }
}
