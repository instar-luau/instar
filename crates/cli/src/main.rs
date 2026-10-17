//! Instar's command-line interface.

mod arguments;
mod check;
mod execution;
mod format;
mod lint;
mod report;
mod watch;
mod workers;

use std::process::ExitCode;

use clap::Parser as _;

use self::arguments::Arguments;

fn main() -> ExitCode {
    let arguments = Arguments::parse();

    match report::initialize(&arguments).and_then(|()| execution::run(&arguments)) {
        Ok(status) => ExitCode::from(status),

        Err(error) => {
            report::failure(&error);

            ExitCode::from(2)
        }
    }
}
