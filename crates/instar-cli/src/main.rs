mod analyze;
mod build;
mod format;
mod input;
mod lint;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "instar", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Analyze(analyze::Analyze),

    Format(format::Format),

    Lint(lint::Lint),

    Lsp,
    Build(build::Build),
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Analyze(arguments) => match arguments.run() {
            Ok(status) => status,

            Err(error) => {
                eprintln!("analyze: {error}");

                ExitCode::FAILURE
            }
        },

        Command::Format(arguments) => match arguments.run() {
            Ok(status) => status,

            Err(error) => {
                eprintln!("format: {error}");

                ExitCode::FAILURE
            }
        },

        Command::Lint(arguments) => match arguments.run() {
            Ok(status) => status,

            Err(error) => {
                eprintln!("lint: {error}");

                ExitCode::FAILURE
            }
        },

        Command::Lsp => match instar_core::lsp::run() {
            Ok(status) => status,

            Err(error) => {
                eprintln!("lsp: {error}");

                ExitCode::FAILURE
            }
        },

        Command::Build(arguments) => match arguments.run() {
            Ok(status) => status,

            Err(error) => {
                eprintln!("build: {error}");

                ExitCode::FAILURE
            }
        },
    }
}
