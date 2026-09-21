//! Command-line entry point for the Instar Luau toolchain.

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
    Analyze,

    /// Prepare or publish compiled project outputs.
    Build,

    /// Format Luau source using inherited project settings.
    Format,

    /// Install graft project dependencies.
    Graft,

    /// Report lint findings or apply safe fixes.
    Lint,

    /// Run the language server over standard input and output.
    Lsp,
}

fn main() -> ExitCode {}
