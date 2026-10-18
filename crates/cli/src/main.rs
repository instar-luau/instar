//! Instar's command-line interface.

mod arguments;
mod association;
mod check;
mod command;
mod execution;
mod format;
mod generate;
mod interrupt;
mod lint;
mod progress;
mod report;
mod watch;
mod workers;

use std::process::ExitCode;

fn main() -> ExitCode {
    command::run()
}
