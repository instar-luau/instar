//! Isolated formatter worker entry point.
use std::io::{self, BufRead, Write};

use instar_analysis::{Options, error::Failure, process::Message};

use crate::{formatter, formatting::Request};

/// Processes formatter requests over standard input and output.
///
/// # Errors
/// Returns malformed protocol or input/output failures.
pub fn run() -> io::Result<()> {
    let input = io::stdin();
    let mut output = io::stdout().lock();

    for line in input.lock().lines() {
        let request: Request = serde_json::from_str(&line?)?;

        let response = formatter::format(
            &request.source,
            request.range,
            &request.configuration,
            &Options::new(request.timeout),
        )
        .map_err(Failure::from);

        serde_json::to_writer(&mut output, &Message::Response(response))?;

        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}
