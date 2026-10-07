use std::{
    collections::BTreeSet,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

use codespan_reporting::term::termcolor::ColorChoice;
use eyre::{Result, WrapErr, ensure};
use instar_analysis::{Completion, Options, Reason};

use instar_core::{
    analysis::{Entry, Origin},
    project::Project,
};

use rayon::prelude::*;

use crate::{
    arguments::{Arguments, Color, Command},
    inputs,
    reporting::Reporter,
    writing,
};

#[derive(Debug, thiserror::Error)]
#[error("analysis incomplete: {0:?}")]
struct Incomplete(Reason);

#[derive(Clone, Copy)]
enum Mode {
    Check,
    Write,
    Stdout,
}

pub(crate) fn run(arguments: &Arguments) -> Result<bool> {
    let mut options = Options::new(Duration::from_secs(arguments.timeout));
    let started = Instant::now();
    let mut project = Project::new(options.timeout);
    let mut changed = Vec::new();

    let color = match arguments.color {
        Color::Always => ColorChoice::Always,
        Color::Never => ColorChoice::Never,

        Color::Auto
            if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
                || !io::stderr().is_terminal() =>
        {
            ColorChoice::Never
        }

        Color::Auto => ColorChoice::Auto,
    };

    console::set_colors_enabled_stderr(!matches!(color, ColorChoice::Never));
    let mut reporter = Reporter::new(arguments.json, color);

    if arguments.verbose {
        reporter.trace()?;
    }

    reporter.phase("Discovering", None)?;
    options.progress = Some(reporter.observer()?);

    let (Command::Check { files } | Command::Lint { files } | Command::Format { files, .. }) =
        &arguments.command;

    let (paths, stdin) = inputs::load(
        files,
        arguments.stdin_filepath.as_deref(),
        &mut project,
        &options,
        started,
    )?;

    let completion = match &arguments.command {
        Command::Format { check, jobs, .. } => {
            ensure!(
                !arguments.json || !stdin || *check,
                "JSON formatting output from stdin requires --check"
            );

            let mode = if *check {
                Mode::Check
            } else if stdin {
                Mode::Stdout
            } else {
                Mode::Write
            };

            let inputs = prepare(paths, &mut project, &options, started, &mut reporter)?;

            let (completion, paths) =
                format(&inputs, *jobs, &options, started, &mut reporter, mode)?;

            changed = paths;

            completion
        }

        Command::Check { .. } => {
            let entries = paths.into_iter().map(Entry::new).collect::<Vec<_>>();
            reporter.phase("Checking", None)?;

            let result = project.check(&entries, &options.remaining(started))?;

            reporter.processed = result
                .modules
                .iter()
                .map(Origin::source)
                .collect::<BTreeSet<_>>()
                .len();

            for diagnostic in result.diagnostics {
                reporter.diagnostic(&mut project, &diagnostic.into())?;
                reporter.flush()?;
            }

            result.completion
        }

        Command::Lint { .. } => {
            let entries = paths.into_iter().map(Entry::new).collect::<Vec<_>>();
            reporter.phase("Linting", None)?;

            let result = project.lint(&entries, &options.remaining(started))?;

            reporter.processed = result
                .modules
                .iter()
                .map(Origin::source)
                .collect::<BTreeSet<_>>()
                .len();

            for diagnostic in result.diagnostics {
                reporter.diagnostic(&mut project, &diagnostic)?;
                reporter.flush()?;
            }

            result.completion
        }
    };

    reporter.finish(&arguments.command, completion, &changed)?;

    if let Completion::Incomplete(reason) = completion {
        return Err(Incomplete(reason).into());
    }

    Ok(!reporter.failed
        && (!matches!(arguments.command, Command::Format { check: true, .. })
            || changed.is_empty()))
}

fn prepare(
    paths: Vec<PathBuf>,
    project: &mut Project,
    options: &Options,
    started: Instant,
    reporter: &mut Reporter,
) -> Result<Vec<(PathBuf, String, instar_format::Configuration)>> {
    reporter.phase("Preparing", Some(paths.len()))?;
    let progress = reporter.progress();
    let mut inputs = Vec::new();

    for path in paths {
        options.check(started)?;
        progress.set_message(path.display().to_string());
        let settings = project.configuration(path.parent().expect("absolute source directory"))?;
        let configuration = settings.configuration.format.clone();

        if !instar_analysis::selection::matches(&path, configuration.include.as_deref(), true)?
            || instar_analysis::selection::matches(&path, configuration.exclude.as_deref(), false)?
        {
            reporter.skipped += 1;
        } else {
            let source = project.source(&path)?.text.to_string();
            inputs.push((path, source, configuration));
        }

        progress.inc(1);
    }

    Ok(inputs)
}

fn format(
    inputs: &[(PathBuf, String, instar_format::Configuration)],
    jobs: Option<std::num::NonZeroUsize>,
    options: &Options,
    started: Instant,
    reporter: &mut Reporter,
    mode: Mode,
) -> Result<(Completion, Vec<String>)> {
    reporter.phase("Formatting", Some(inputs.len()))?;
    let progress = reporter.progress();
    let mut builder = rayon::ThreadPoolBuilder::new();

    if let Some(jobs) = jobs {
        builder = builder.num_threads(jobs.get());
    }

    let pool = builder.build()?;
    let mut completion = Completion::Complete;
    let mut changed = Vec::new();

    for batch in inputs.chunks(pool.current_num_threads()) {
        let results = pool.install(|| {
            batch
                .par_iter()
                .map(|(path, source, configuration)| {
                    progress.set_message(path.display().to_string());

                    instar_format::format(
                        source.as_bytes(),
                        configuration,
                        &options.remaining(started),
                    )
                    .wrap_err_with(|| format!("formatting {}", path.display()))
                })
                .collect::<Vec<_>>()
        });

        for ((path, source, _), result) in batch.iter().zip(results) {
            let result = result?;
            completion = completion.combine(result.completion);
            reporter.formatting(path, source, &result);
            reporter.flush()?;

            if result.completion == Completion::Complete
                && result.diagnostics.is_empty()
                && let Some(output) = result.output
            {
                match mode {
                    Mode::Check => {}
                    Mode::Write => writing::replace(path, source.as_bytes(), &output)?,
                    Mode::Stdout => io::stdout().lock().write_all(&output)?,
                }

                if output == source.as_bytes() {
                    reporter.unchanged += 1;
                } else {
                    reporter.formatted += 1;

                    if matches!(mode, Mode::Check) {
                        reporter.difference(path);
                    }

                    changed.push(path.display().to_string());
                }
            } else {
                reporter.unsuccessful += 1;
            }

            reporter.processed += 1;
            progress.inc(1);
        }

        if completion != Completion::Complete {
            break;
        }
    }

    Ok((completion, changed))
}
