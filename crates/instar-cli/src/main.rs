mod analyze;
mod format;
mod input;

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

    Lint {
        #[arg(long)]
        fix: bool,
    },

    Lsp,
    Build,
}

fn main() -> ExitCode {
    let command = match Cli::parse().command {
        Command::Analyze(arguments) => {
            return match arguments.run() {
                Ok(status) => status,

                Err(error) => {
                    eprintln!("analyze: {error}");

                    ExitCode::FAILURE
                }
            };
        }

        Command::Format(arguments) => {
            return match arguments.run() {
                Ok(status) => status,

                Err(error) => {
                    eprintln!("format: {error}");

                    ExitCode::FAILURE
                }
            };
        }

        Command::Lint { fix: false } => "lint",
        Command::Lint { fix: true } => "lint --fix",

        Command::Lsp => {
            return match instar_core::lsp::run() {
                Ok(status) => status,

                Err(error) => {
                    eprintln!("lsp: {error}");

                    ExitCode::FAILURE
                }
            };
        }

        Command::Build => "build",
    };

    eprintln!("{command}: not implemented");

    ExitCode::FAILURE
}
