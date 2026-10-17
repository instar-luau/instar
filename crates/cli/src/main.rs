//! Instar's command-line interface.

mod arguments;
mod check;
mod execution;
mod format;
mod interrupt;
mod lint;
mod progress;
mod report;
mod schema;
mod watch;
mod workers;

use std::process::ExitCode;

use clap::Parser as _;

use self::arguments::{Arguments, Command};

fn main() -> ExitCode {
    let arguments = Arguments::parse();

    let result = report::initialize(arguments.color, arguments.verbose).and_then(|()| {
        match &arguments.command {
            Command::Schema => schema::run(),

            Command::Files(operation) => {
                interrupt::initialize()?;
                let result = execution::run(&arguments, operation);

                if interrupt::requested() {
                    report::interrupted(&arguments, operation);

                    return Ok(if operation.options().watch { 0 } else { 130 });
                }

                result
            }
        }
    });

    match result {
        Ok(status) => ExitCode::from(status),

        Err(error) => {
            report::failure(&error);

            ExitCode::from(2)
        }
    }
}
