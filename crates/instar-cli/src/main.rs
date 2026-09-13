//! Command-line entry point for the Instar Luau toolchain.

mod analyze;
mod build;
mod format;
mod input;
mod lint;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Analyze, build, format, and lint Luau projects.
#[derive(Parser)]
#[command(name = "instar", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check Luau source and optionally emit inferred type annotations.
    Analyze(analyze::Analyze),

    /// Prepare or publish compiled project outputs.
    Build(build::Build),

    /// Format Luau source using inherited project settings.
    Format(format::Format),

    /// Report lint findings or apply safe fixes.
    Lint(lint::Lint),

    /// Run the language server over standard input and output.
    Lsp,
}

fn main() -> ExitCode {
    let (name, result) = match Cli::parse().command {
        Command::Analyze(arguments) => ("analyze", arguments.run()),
        Command::Build(arguments) => ("build", arguments.run()),
        Command::Format(arguments) => ("format", arguments.run()),
        Command::Lint(arguments) => ("lint", arguments.run()),
        Command::Lsp => ("lsp", instar_core::lsp::run()),
    };

    match result {
        Ok(status) => status,

        Err(error) => {
            eprintln!("{name}: {error}");

            ExitCode::FAILURE
        }
    }
}
