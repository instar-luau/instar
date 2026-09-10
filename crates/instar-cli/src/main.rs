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
    Analyze,
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
        Command::Analyze => "analyze",
        Command::Format => "format",
        Command::Lint { fix: false } => "lint",
        Command::Lint { fix: true } => "lint --fix",
        Command::Lsp => "lsp",
        Command::Build => "build",
    };
    eprintln!("{command}: not implemented");
    ExitCode::FAILURE
}
