//! Instar's command-line interface.

mod arguments;
mod check;
mod execution;
mod format;
mod lint;
mod reporting;
mod watching;
mod workers;

use std::process::ExitCode;

use arguments::Arguments;
use clap::Parser as _;

fn main() -> ExitCode {
    let arguments = Arguments::parse();

    match reporting::initialize(&arguments).and_then(|()| execution::run(&arguments)) {
        Ok(status) => ExitCode::from(status),

        Err(error) => {
            reporting::failure(&error);

            ExitCode::from(2)
        }
    }
}
