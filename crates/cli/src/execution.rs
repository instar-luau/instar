use std::{
    collections::{BTreeMap, BTreeSet},
    env, io, mem,
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::Instant,
};

use eyre::{Report, Result, WrapErr as _, bail};

use instar_core::workspace::{Access, Generators, Purpose, Workspace};

use super::{
    arguments::{Arguments, Operation, Options},
    batch::Batch,
    check, format, interrupt, lint,
    output::{self, Progress},
    watch::{Change, Watcher},
};

pub(super) struct Project {
    pub(super) workspace: Workspace,
    pub(super) generation: Option<Generators>,
    pub(super) overrides: Vec<String>,
    pub(super) selected: BTreeMap<String, PathBuf>,
    pub(super) stdin_module: Option<String>,
    pub(super) stdin: Option<(PathBuf, Arc<str>)>,
    pub(super) progress: Progress,
}

pub(super) enum Response {
    Completed(Batch),
    Reload { configuration: bool },
}

pub(super) enum Outcome {
    Completed(u8),

    Reload { started: Instant },
}

pub(super) fn run(arguments: &Arguments, operation: &Operation) -> Result<u8> {
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

    if stdin_count == 0 && arguments.stdin_filepath.is_some() {
        return Err(output::Validation::new(
            "--stdin-filepath requires stdin input.",
            "Add - to the input list.",
        )
        .into());
    }

    if options.watch && stdin_count != 0 {
        return Err(output::Validation::new(
            "Watch mode cannot read from stdin.",
            "Select files or directories, or remove --watch.",
        )
        .into());
    }

    let overrides = operation.overrides()?;

    let stdin = if stdin_count == 1 {
        Some((
            arguments
                .stdin_filepath
                .clone()
                .unwrap_or_else(|| PathBuf::from("stdin.luau")),
            Arc::from(input()?),
        ))
    } else {
        None
    };

    let mut watcher = options
        .watch
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

pub(super) fn shutdown(generators: &Generators) -> Result<()> {
    generators.shutdown().map_err(|error| {
        let error = Report::new(error);

        if interrupt::requested() {
            output::failure(&error);
        }

        error
    })
}

pub(super) fn session(
    operation: &Operation,
    project: &Project,
    mut watcher: Option<&mut Watcher>,
    mut started: Instant,
    mut execute: impl FnMut(
        &BTreeMap<String, PathBuf>,
        &BTreeSet<String>,
        bool,
        &Progress,
    ) -> Result<Response>,
) -> Result<Outcome> {
    let mut dirty = BTreeSet::new();
    let mut selected = project.selected.clone();
    let mut discovery = false;

    if let Some(watcher) = &mut watcher {
        watcher.begin(&project.workspace)?;
    }

    loop {
        tracing::debug!("Processing batch");

        let progress = &project.progress;
        progress.reset(selected.len());
        let result = execute(&selected, &dirty, discovery, progress);
        progress.clear();
        interrupt::check()?;

        let mut batch = match result? {
            Response::Completed(batch) => batch,

            Response::Reload { configuration } => {
                if configuration && let Some(watcher) = &mut watcher {
                    watcher.reset()?;
                }

                return Ok(Outcome::Reload { started });
            }
        };

        if let Some(watcher) = &mut watcher {
            batch.dependencies.extend(project.workspace.dependencies());

            batch
                .reload_dependencies
                .extend(project.workspace.reload_dependencies());

            watcher.observe(mem::take(&mut batch.reload_dependencies), true)?;
            watcher.observe(mem::take(&mut batch.dependencies), false)?;
        }

        let result = batch.publish(operation, project);
        progress.clear();
        let summary = result?;

        output::summary(&summary, operation, progress.files(), started);
        let status = summary.status(operation);

        let Some(watcher) = &mut watcher else {
            return Ok(Outcome::Completed(status));
        };

        output::watching();

        loop {
            let (paths, structural, changed) = match watcher.changed(&project.workspace)? {
                Change::Sources {
                    paths,
                    discovery,
                    started,
                } => (paths, discovery, started),

                Change::Reload { started } => {
                    return Ok(Outcome::Reload { started });
                }
            };

            started = changed;

            let previous = if structural {
                mem::replace(
                    &mut selected,
                    select(
                        operation,
                        &project.workspace,
                        project.stdin_module.as_deref(),
                    )?,
                )
            } else {
                BTreeMap::new()
            };

            let mut candidates = project.workspace.dependencies();
            candidates.extend(selected.keys().chain(previous.keys()).map(PathBuf::from));
            dirty.clear();
            discovery = false;

            for candidate in candidates {
                let module = project.workspace.normalize(&candidate)?;

                let membership =
                    structural && selected.contains_key(&module) != previous.contains_key(&module);

                if !membership && !paths.iter().any(|path| candidate.starts_with(path)) {
                    continue;
                }

                discovery |= structural;

                if candidate
                    .extension()
                    .is_some_and(|extension| extension == "lua" || extension == "luau")
                    && !candidate.is_dir()
                    && (project.workspace.refresh(&module)? || membership)
                {
                    dirty.insert(module);
                }
            }

            if !dirty.is_empty() || discovery || (structural && selected != previous) {
                if structural {
                    watcher.begin(&project.workspace)?;
                }

                break;
            }

            tracing::debug!("Ignoring filesystem changes outside active inputs");
        }
    }
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

pub(super) fn open(
    root: &Path,
    options: &Options,
    overrides: &[String],
    stdin: Option<&(PathBuf, Arc<str>)>,
) -> Result<(Workspace, Option<String>)> {
    let workspace = Workspace::new(root, &options.configuration, overrides)?
        .with_cancellation(interrupt::requested);

    let module = stdin
        .map(|(path, source)| workspace.set_source(path, Arc::clone(source)))
        .transpose()?;

    Ok((workspace, module))
}

fn prepare(
    operation: &Operation,
    root: &Path,
    overrides: &[String],
    stdin: Option<&(PathBuf, Arc<str>)>,
    watcher: Option<&mut Watcher>,
) -> Result<Project> {
    let options = operation.options();
    let (mut workspace, stdin_module) = open(root, options, overrides, stdin)?;
    let selected = select(operation, &workspace, stdin_module.as_deref())?;
    let progress = Progress::new(operation, workspace.directory())?;

    let generation = if matches!(operation, Operation::Format(_)) {
        None
    } else {
        watcher.as_ref().map(|watcher| watcher.generators.clone())
    };

    if let Some(generation) = &generation {
        workspace = workspace.with_generators(generation.clone());
    }

    if !matches!(operation, Operation::Format(_)) {
        if let Some(watcher) = watcher {
            watcher.observe(workspace.reload_dependencies(), true)?;
        }

        let mut generated = BTreeMap::new();

        for generator in workspace.generators() {
            let source = workspace.generate(&generator)?;
            generated.insert(generator, source);
        }

        workspace.set_generated(generated);
    }

    Ok(Project {
        workspace,
        generation,
        overrides: overrides.to_owned(),
        selected,
        stdin_module,
        stdin: stdin.cloned(),
        progress,
    })
}

fn select(
    operation: &Operation,
    workspace: &Workspace,
    stdin_module: Option<&str>,
) -> Result<BTreeMap<String, PathBuf>> {
    let options = operation.options();

    let inputs: Vec<_> = options
        .inputs
        .iter()
        .filter(|path| path.as_os_str() != "-")
        .cloned()
        .collect();

    let (purpose, access) = match operation {
        Operation::Check(_) => (Purpose::Check, Access::Read),
        Operation::Format(_) => (Purpose::Format, Access::Write),

        Operation::Lint(lint) => (
            Purpose::Lint,
            if lint.corrections {
                Access::Write
            } else {
                Access::Read
            },
        ),
    };

    let mut selected = if stdin_module.is_some() && inputs.is_empty() {
        BTreeMap::new()
    } else {
        workspace.select(&inputs, purpose, access)?
    };

    if let Some(module) = stdin_module {
        selected.insert(module.to_owned(), PathBuf::from(module));
    }

    Ok(selected)
}
