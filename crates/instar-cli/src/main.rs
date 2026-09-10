mod analyze;
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
    /// Analyze files and directories with pinned Luau.
    Analyze(analyze::Analyze),

    Format,

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

        Command::Format => "format",
        Command::Lint { fix: false } => "lint",
        Command::Lint { fix: true } => "lint --fix",
        Command::Lsp => "lsp",
        Command::Build => "build",
    };

    eprintln!("{command}: not implemented");

    ExitCode::FAILURE
}