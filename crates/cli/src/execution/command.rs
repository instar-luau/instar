use std::{
    env, io,
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Instant,
};

use eyre::{Result, WrapErr as _, bail};

use super::{
    project::{prepare, shutdown},
    session::Outcome,
    watch::Watcher,
};

use crate::{arguments::Operation, check, format, interrupt, lint, output};

pub(crate) fn run(operation: &Operation) -> Result<u8> {
    let mut started = Instant::now();

    let options = operation.options();

    let root = options
        .root
        .clone()
        .map_or_else(env::current_dir, Ok)
        .wrap_err("Could not resolve the working directory.")?;

    let root = root
        .canonicalize()
        .wrap_err_with(|| format!("Could not resolve project root `{}`.", root.display()))?;

    if !root.is_dir() {
        bail!("Project root `{}` must be a directory.", root.display());
    }

    let stdin_count = options
        .inputs
        .iter()
        .filter(|path| path.as_os_str() == "-")
        .count();

    if stdin_count > 1 {
        return Err(output::Validation::new(
            "Stdin (-) can be selected only once.",
            "Remove the repeated - input.",
        )
        .into());
    }

    if stdin_count == 0 && options.stdin_filepath.is_some() {
        return Err(output::Validation::new(
            "--stdin-filepath requires stdin input.",
            "Add - to the input list.",
        )
        .into());
    }

    if options.should_watch && stdin_count != 0 {
        return Err(output::Validation::new(
            "Watch mode cannot read from stdin.",
            "Select files or directories, or remove --watch.",
        )
        .into());
    }

    let overrides = operation.overrides()?;

    let stdin = if stdin_count == 1 {
        Some((
            options
                .stdin_filepath
                .clone()
                .unwrap_or_else(|| PathBuf::from("stdin.luau")),
            Arc::from(input()?),
        ))
    } else {
        None
    };

    let mut watcher = options
        .should_watch
        .then(|| Watcher::new(&root, options))
        .transpose()?;

    let result = (|| {
        loop {
            interrupt::check()?;

            let result = prepare(
                operation,
                &root,
                &overrides,
                stdin.as_ref(),
                watcher.as_mut(),
            )
            .and_then(|project| match operation {
                Operation::Check(_) => {
                    check::session(operation, &project, watcher.as_mut(), started)
                }

                Operation::Format(_) => {
                    format::session(operation, &project, watcher.as_mut(), started)
                }

                Operation::Lint(_) => lint::session(operation, &project, watcher.as_mut(), started),
            });

            interrupt::check()?;

            match result {
                Ok(Outcome::Completed(status)) => return Ok(status),
                Ok(Outcome::Reload { started: reloaded }) => started = reloaded,

                Err(error) => {
                    let Some(watcher) = &mut watcher else {
                        return Err(error);
                    };

                    output::failure(&error);
                    watcher.reset()?;
                    output::watching();
                    watcher.wait(true)?;
                    started = Instant::now();
                }
            }
        }
    })();

    if let Some(watcher) = watcher {
        shutdown(&watcher.generators)?;
    }

    result
}

fn input() -> Result<String> {
    let (sender, receiver) = mpsc::channel();

    drop(
        thread::Builder::new()
            .spawn(move || {
                drop(sender.send(io::read_to_string(io::stdin().lock())));
            })
            .wrap_err("Could not start the stdin reader.")?,
    );

    loop {
        interrupt::check()?;

        match receiver.recv_timeout(interrupt::INTERVAL) {
            Ok(result) => return result.wrap_err("Could not read stdin."),
            Err(RecvTimeoutError::Timeout) => {}

            Err(error) => {
                return Err(error).wrap_err("Could not receive input from the stdin reader.");
            }
        }
    }
}
