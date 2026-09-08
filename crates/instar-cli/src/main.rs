use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// The Instar Luau toolchain.
#[derive(Parser)]
#[command(name = "instar", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Analyze project types with Luau (not implemented).
    Analyze,
    /// Format source with the canonical style (not implemented).
    Format,
    /// Report lint findings and optionally apply fixes (not implemented).
    Lint {
        /// Apply automatic lint fixes.
        #[arg(long)]
        fix: bool,
    },
    /// Start editor services, including formatting and linting (not implemented).
    Lsp,
    /// Transform, refactor, generate, minify and bundle source (not implemented).
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
