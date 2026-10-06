//! Isolated formatter worker entry point.

use std::io::{self, BufRead, Write};

use crate::formatting::{Request, Response};
use instar_analysis::Options;

/// Processes formatter requests over standard input and output.
///
/// # Errors
/// Returns malformed protocol or input/output failures.
pub fn run() -> io::Result<()> {
    let input = io::stdin();
    let mut output = io::stdout().lock();

    for line in input.lock().lines() {
        let request: Request = serde_json::from_str(&line?)?;

        let response = match crate::formatter::format(
            &request.source,
            &request.configuration,
            &Options::new(request.timeout),
        ) {
            Ok(result) => Response::Formatted(result),
            Err(error) => Response::InvalidConfiguration(error.to_string()),
        };

        serde_json::to_writer(&mut output, &Ok::<_, String>(response))?;
        output.write_all(b"\n")?;
        output.flush()?;
    }

    Ok(())
}
