//! Command-line entry point for the Instar Luau toolchain.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    process::ExitCode,
};

use clap::{Parser, Subcommand};
use instar_core::{analysis, filter::Service, format, project::Project};

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
        /// Source files or directories.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },

    /// Format Luau source files.
    Format {
        /// Source files or directories.
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },

    /// Run the language server over standard input and output.
    Lsp,
}

fn expand_paths(
    paths: Vec<PathBuf>,
    project: &mut Project,
    service: Service,
) -> io::Result<Vec<PathBuf>> {
    let mut pending = paths;
    let mut files = BTreeMap::new();

    while let Some(path) = pending.pop() {
        let metadata = fs::metadata(&path)?;

        if metadata.is_dir() {
            if project.excludes_subtree(&path, service)? {
                continue;
            }

            for entry in fs::read_dir(&path)? {
                let entry = entry?;
                let kind = entry.file_type()?;
                let source = entry.path();

                if kind.is_dir() && entry.file_name() != ".git" {
                    pending.push(source);
                } else if kind.is_file()
                    && matches!(
                        source.extension().and_then(|extension| extension.to_str()),
                        Some("lua" | "luau")
                    )
                {
                    files
                        .entry(instar_core::absolute(&source)?)
                        .or_insert(source);
                }
            }
        } else if metadata.is_file() {
            files.entry(instar_core::absolute(&path)?).or_insert(path);
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("not a file or directory: {}", path.display()),
            ));
        }
    }

    if files.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no eligible .lua or .luau files found",
        ));
    }

    Ok(files.into_values().collect())
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Lsp => instar_server::run().map(|()| false),

        Command::Analyze { paths } => {
            let mut project = Project::new();

            let result = expand_paths(paths, &mut project, Service::Analyze)
                .and_then(|paths| analysis::check(&mut project, &paths));

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

            expand_paths(paths, &mut project, Service::Format).and_then(|paths| {
                let mut excluded = 0;

                let result = paths
                    .into_iter()
                    .try_for_each(|path| -> std::io::Result<()> {
                        let Some(options) = project.format_options(&path)? else {
                            excluded += 1;

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
                    });

                if excluded != 0 {
                    eprintln!(
                        "skipped {excluded} {} excluded by format filters",
                        if excluded == 1 { "file" } else { "files" }
                    );
                }

                result.map(|()| false)
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
