//! Instar's command-line interface.

mod arguments;
mod assets;
mod batch;
mod check;
mod command;
mod execution;
mod format;
mod interrupt;
mod lint;
mod output;
mod paths;

use self::command::main;
