use std::{
    collections::{BTreeMap, BTreeSet},
    env,
    io::{self, IsTerminal, Write},
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

use codespan_reporting::term::termcolor::ColorChoice;
use eyre::{Result, WrapErr, ensure};
use instar_analysis::{Completion, Options, Reason, process, selection};

use instar_core::{
    analysis::{Entry, Origin},
    project::Project,
};

use instar_format::{Configuration, Formatter};

use crate::{
    arguments::{Analysis, Arguments, Color},
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

pub(crate) fn run(arguments: &Arguments, command: &Analysis) -> Result<bool> {
    process::set_host(&env::current_exe()?)?;

    let mut options = Options::new(Duration::from_secs(arguments.timeout));
    let started = Instant::now();
    let mut project = Project::new(options.timeout);
    let mut changed = Vec::new();

    let color = match arguments.color {
        Color::Always => ColorChoice::Always,
        Color::Never => ColorChoice::Never,

        Color::Auto
            if env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty())
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

    let (Analysis::Check { files } | Analysis::Lint { files, .. } | Analysis::Format { files, .. }) =
        command;

    let (paths, stdin) = inputs::load(
        files,
        arguments.stdin_filepath.as_deref(),
        &mut project,
        &options,
        started,
    )?;

    let completion = match command {
        Analysis::Format { check, .. } => {
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

            let inputs = prepare(paths, mode, &mut project, &mut reporter, &options, started)?;

            let (completion, paths) = format(&inputs, mode, &mut reporter, &options, started)?;

            changed = paths;

            completion
        }

        Analysis::Check { .. } => {
            let entries = entries(paths, &mut project, &options, started)?;
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

        Analysis::Lint { .. } => {
            let entries = entries(paths, &mut project, &options, started)?;
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

    reporter.finish(command, completion, &changed)?;

    if let Completion::Incomplete(reason) = completion {
        return Err(Incomplete(reason).into());
    }

    Ok(!reporter.failed
        && (!matches!(command, Analysis::Format { check: true, .. }) || changed.is_empty()))
}

fn entries(
    paths: Vec<PathBuf>,
    project: &mut Project,
    options: &Options,
    started: Instant,
) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();

    for path in paths {
        options.check(started)?;

        entries.extend(project.contexts(&path)?.into_iter().map(|module| Entry {
            source: module.source,
            context: Some(module.identity),
        }));
    }

    Ok(entries)
}

fn prepare(
    paths: Vec<PathBuf>,
    mode: Mode,
    project: &mut Project,
    reporter: &mut Reporter,
    options: &Options,
    started: Instant,
) -> Result<Vec<(PathBuf, Arc<str>, Configuration)>> {
    reporter.phase("Preparing", Some(paths.len()))?;
    let progress = reporter.progress();
    let mut inputs = Vec::new();

    for path in paths {
        options.check(started)?;
        progress.set_message(path.display().to_string());
        let settings = project.configuration(path.parent().expect("absolute source directory"))?;
        let configuration = settings.configuration.format.clone();

        if !selection::matches(&path, configuration.include.as_deref(), true)?
            || selection::matches(&path, configuration.exclude.as_deref(), false)?
        {
            reporter.skipped += 1;

            if matches!(mode, Mode::Stdout) {
                io::stdout()
                    .lock()
                    .write_all(project.source(&path)?.text.as_bytes())?;
            }
        } else {
            let source = project.source(&path)?.text;
            inputs.push((path, source, configuration));
        }

        progress.inc(1);
    }

    Ok(inputs)
}

fn format(
    inputs: &[(PathBuf, Arc<str>, Configuration)],
    mode: Mode,
    reporter: &mut Reporter,
    options: &Options,
    started: Instant,
) -> Result<(Completion, Vec<String>)> {
    reporter.phase("Formatting", Some(inputs.len()))?;
    let progress = reporter.progress();
    let pool = rayon::ThreadPoolBuilder::new().build()?;
    let mut completion = Completion::Complete;
    let mut changed = Vec::new();
    let (dispatch, receive) = mpsc::channel::<usize>();
    let receive = Mutex::new(receive);
    let (send, results) = mpsc::channel();

    let window = pool
        .current_num_threads()
        .saturating_mul(2)
        .min(inputs.len());

    pool.in_place_scope(|scope| -> Result<()> {
        for _ in 0..pool.current_num_threads().min(inputs.len()) {
            let send = send.clone();
            let receive = &receive;
            let progress = &progress;

            scope.spawn(move |_| {
                let mut worker = Formatter::default();

                loop {
                    let Ok(index) = receive.lock().expect("format dispatch lock").recv() else {
                        break;
                    };

                    let (path, source, configuration) = &inputs[index];
                    progress.set_message(path.display().to_string());

                    let result = worker
                        .format(
                            source.as_bytes(),
                            configuration,
                            &options.remaining(started),
                        )
                        .wrap_err_with(|| format!("formatting {}", path.display()));

                    if send.send((index, result)).is_err() {
                        break;
                    }
                }
            });
        }

        drop(send);
        let dispatch = dispatch;

        for index in 0..window {
            dispatch.send(index)?;
        }

        let mut pending = BTreeMap::new();

        for (index, (path, source, _)) in inputs.iter().enumerate() {
            while !pending.contains_key(&index) {
                let (finished, result) = results.recv()?;
                pending.insert(finished, result);
            }

            let result = pending.remove(&index).expect("ordered format result");
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

            if completion != Completion::Complete {
                break;
            }

            if index + window < inputs.len() {
                dispatch.send(index + window)?;
            }
        }

        Ok(())
    })?;

    Ok((completion, changed))
}
