//! Instar's command-line interface.

mod arguments;
mod check;
mod execution;
mod format;
mod interrupt;
mod lint;
mod progress;
mod report;
mod watch;
mod workers;

use std::process::ExitCode;

use clap::Parser as _;

use self::arguments::Arguments;

fn main() -> ExitCode {
    let arguments = Arguments::parse();

    let result = report::initialize(&arguments)
        .and_then(|()| interrupt::initialize())
        .and_then(|()| execution::run(&arguments));

    if interrupt::requested() {
        report::interrupted(&arguments);

        return ExitCode::from(if arguments.command.options().watch {
            0
        } else {
            130
        });
    }

    match result {
        Ok(status) => ExitCode::from(status),

        Err(error) => {
            report::failure(&error);

            ExitCode::from(2)
        }
    }
}
