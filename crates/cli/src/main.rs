//! Instar's command-line interface.

mod arguments;
mod association;
mod batch;
mod check;
mod command;
mod execution;
mod format;
mod generate;
mod interrupt;
mod lint;
mod output;
mod watch;
mod workers;

use self::command::main;
