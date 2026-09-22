//! Command-line entry point for the Instar Luau toolchain.

use std::{path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};
use instar_core::{analysis, project::Project};

/// Analyze Luau projects and serve editor requests.
#[derive(Parser)]
#[command(name = "instar", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Check Luau source and its dependencies.
    Analyze {
        /// Entry source files.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },

    /// Run the language server over standard input and output.
    Lsp,
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Lsp => instar_server::run().map(|()| false),

        Command::Analyze { paths } => {
            let mut project = Project::new();
            let result = analysis::check(&mut project, &paths);

            for warning in project.take_asset_warnings() {
                eprintln!("warning: {warning}");
            }

            result.map(|diagnostics| {
                for diagnostic in &diagnostics {
                    println!(
                        "{}:{}:{}: {}: {}",
                        diagnostic.location.module.source.display(),
                        diagnostic.location.range[0] + 1,
                        diagnostic.location.range[1] + 1,
                        if diagnostic.error { "error" } else { "warning" },
                        diagnostic.message
                    );
                }

                diagnostics.iter().any(|diagnostic| diagnostic.error)
            })
        }
    };

    match result {
        Ok(false) => ExitCode::SUCCESS,
        Ok(true) => ExitCode::FAILURE,

        Err(error) => {
            eprintln!("error: {error}");

            ExitCode::from(2)
        }
    }
}
