//! Command-line entry point for the Instar Luau toolchain.

use std::{
    fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use instar_core::{analysis, format, project::Project};

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

    /// Format Luau source files.
    Format {
        /// Source files to format.
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

        Command::Format { paths } => {
            let mut project = Project::new();

            paths
                .into_iter()
                .try_for_each(|path| -> std::io::Result<()> {
                    let Some(options) = project.format_options(&path)? else {
                        eprintln!("skipped {}: excluded by format filters", path.display());

                        return Ok(());
                    };

                    let source = fs::read_to_string(&path)?;

                    let formatted = format::source(&source, &options).map_err(|failure| {
                        std::io::Error::new(
                            failure.kind(),
                            format!("{}: {failure}", path.display()),
                        )
                    })?;

                    if source != formatted {
                        atomic_write(&fs::canonicalize(&path)?, formatted.as_bytes())?;
                    }

                    Ok(())
                })
                .map(|()| false)
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

fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));

    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("source path has no file name"))?;

    for attempt in 0..100 {
        let temporary = parent.join(format!(
            ".{}.{}.{}.tmp",
            name.to_string_lossy(),
            std::process::id(),
            attempt
        ));

        let mut file = match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(failure) => return Err(failure),
        };

        let result = (|| {
            use std::io::Write;
            file.write_all(contents)?;
            file.sync_all()?;

            if let Ok(metadata) = fs::metadata(path) {
                fs::set_permissions(&temporary, metadata.permissions())?;
            }

            fs::rename(&temporary, path)
        })();

        if result.is_err() {
            drop(fs::remove_file(&temporary));
        }

        return result;
    }

    Err(std::io::Error::other("cannot create temporary file"))
}
