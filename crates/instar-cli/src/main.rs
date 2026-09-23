//! Command-line entry point for the Instar Luau toolchain.

use std::{
    collections::BTreeMap,
    fs,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    process::ExitCode,
    time::{Duration, Instant},
};

use clap::{Parser, Subcommand};
use console::Style;
use indicatif::{ProgressBar, ProgressStyle};
use instar_core::{analysis, filter::Service, format, project::Project};

/// Analyze Luau projects and serve editor requests.
#[derive(Parser)]
#[command(name = "instar", version)]
struct Cli {
    /// Disable animations and color; keep stable text summaries.
    #[arg(long, global = true)]
    plain: bool,

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
    let Cli { command, plain } = Cli::parse();
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());

    let stdout_color =
        !plain && !no_color && io::stdout().is_terminal() && console::colors_enabled();

    let stderr_color =
        !plain && !no_color && io::stderr().is_terminal() && console::colors_enabled_stderr();

    let result = match command {
        Command::Lsp => instar_server::run().map(|()| false),
        Command::Analyze { paths } => run_analyze(paths, plain, stdout_color, stderr_color),
        Command::Format { paths } => run_format(paths, plain, stderr_color),
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

fn run_analyze(
    paths: Vec<PathBuf>,
    plain: bool,
    stdout_color: bool,
    stderr_color: bool,
) -> io::Result<bool> {
    let started = Instant::now();
    let progress = progress(plain, stderr_color);
    let mut project = Project::new();

    let result = expand_paths(paths, &mut project, Service::Analyze).and_then(|paths| {
        let count = paths.len();
        progress.set_message("Analyzing");

        let diagnostics = if plain {
            analysis::check(&mut project, &paths)?
        } else {
            analysis::check_streaming(
                &mut project,
                &paths,
                |path| {
                    if !progress.is_hidden() {
                        progress.set_message(format!("Analyzing {}", path.display()));
                    }
                },
                |batch| {
                    progress.suspend(|| {
                        for diagnostic in batch {
                            print_diagnostic(diagnostic, stdout_color)?;
                        }

                        Ok(())
                    })
                },
            )?
        };

        Ok((diagnostics, count))
    });

    progress.finish_and_clear();

    for warning in project.take_asset_warnings() {
        eprintln!("warning: {warning}");
    }

    result.and_then(|(diagnostics, count)| {
        if plain {
            for diagnostic in &diagnostics {
                print_diagnostic(diagnostic, false)?;
            }
        }

        let errors = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.error)
            .count();

        let warnings = diagnostics.len() - errors;

        let style = if stderr_color {
            if errors != 0 {
                Style::new().red().for_stderr()
            } else if warnings != 0 {
                Style::new().yellow().for_stderr()
            } else {
                Style::new().green().for_stderr()
            }
        } else {
            Style::new()
        };

        eprintln!(
            "{} {count} {}: {errors} errors, {warnings} warnings in {:.2?}",
            style.apply_to("Analyzed"),
            if count == 1 { "input" } else { "inputs" },
            started.elapsed()
        );

        Ok(diagnostics.iter().any(|diagnostic| diagnostic.error))
    })
}

fn run_format(paths: Vec<PathBuf>, plain: bool, stderr_color: bool) -> io::Result<bool> {
    let started = Instant::now();
    let progress = progress(plain, stderr_color);
    let mut project = Project::new();

    let result = expand_paths(paths, &mut project, Service::Format).and_then(|paths| {
        let count = paths.len();
        show_progress(&progress, "Formatting", count, stderr_color);
        let mut excluded = 0;
        let mut changed = 0;

        let result = paths.into_iter().try_for_each(|path| -> io::Result<()> {
            if !progress.is_hidden() {
                progress.set_message(format!("Formatting {}", path.display()));
            }

            let Some(options) = project.format_options(&path)? else {
                excluded += 1;
                progress.inc(1);

                return Ok(());
            };

            let source = fs::read_to_string(&path)?;

            let formatted = format::source(&source, &options).map_err(|failure| {
                io::Error::new(failure.kind(), format!("{}: {failure}", path.display()))
            })?;

            if source != formatted {
                atomic_write(&fs::canonicalize(&path)?, formatted.as_bytes())?;
                changed += 1;
            }

            progress.inc(1);

            Ok(())
        });

        result.map(|()| (count, changed, excluded))
    });

    progress.finish_and_clear();

    result.map(|(count, changed, excluded)| {
        let style = if stderr_color {
            Style::new().green().for_stderr()
        } else {
            Style::new()
        };

        eprintln!(
            "{} {count} {}: {changed} changed, {} unchanged, {excluded} excluded in {:.2?}",
            style.apply_to("Formatted"),
            if count == 1 { "file" } else { "files" },
            count - changed - excluded,
            started.elapsed()
        );

        false
    })
}

fn progress(plain: bool, color: bool) -> ProgressBar {
    if plain || !io::stderr().is_terminal() {
        return ProgressBar::hidden();
    }

    let progress = ProgressBar::new_spinner();

    let template = if color {
        "{spinner:.cyan} {wide_msg}"
    } else {
        "{spinner} {wide_msg}"
    };

    progress.set_style(ProgressStyle::with_template(template).expect("static progress template"));
    progress.set_message("Discovering files");
    progress.enable_steady_tick(Duration::from_millis(100));

    progress
}

fn show_progress(progress: &ProgressBar, task: &'static str, count: usize, color: bool) {
    progress.set_length(count as u64);
    progress.set_message(task);

    let template = if color {
        "{spinner:.cyan} {pos}/{len} {wide_msg}"
    } else {
        "{spinner} {pos}/{len} {wide_msg}"
    };

    progress.set_style(ProgressStyle::with_template(template).expect("static progress template"));
}

fn print_diagnostic(diagnostic: &analysis::Diagnostic, color: bool) -> io::Result<()> {
    let severity = if diagnostic.error { "error" } else { "warning" };

    let style = if color {
        if diagnostic.error {
            Style::new().red().for_stdout()
        } else {
            Style::new().yellow().for_stdout()
        }
    } else {
        Style::new()
    };

    writeln!(
        io::stdout().lock(),
        "{}:{}:{}: {}: {}",
        diagnostic.location.module.source.display(),
        diagnostic.location.range[0] + 1,
        diagnostic.location.range[1] + 1,
        style.apply_to(severity),
        diagnostic.message
    )
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
