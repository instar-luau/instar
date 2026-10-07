//! Isolated formatter worker entry point.
use std::io::{self, BufRead, Write};

use instar_analysis::Options;

use crate::formatting::Request;

/// Processes formatter requests over standard input and output.
///
/// # Errors
/// Returns malformed protocol or input/output failures.
pub fn run() -> io::Result<()> {
    let input = io::stdin();
    let mut output = io::stdout().lock();

    for line in input.lock().lines() {
        let request: Request = serde_json::from_str(&line?)?;

        let response = crate::formatter::format(
            &request.source,
            &request.configuration,
            &Options::new(request.timeout),
        )
        .map_err(instar_analysis::error::Failure::from);

        serde_json::to_writer(&mut output, &response)?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}
