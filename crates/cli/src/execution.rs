use std::{
    collections::{BTreeMap, BTreeSet},
    env, io, mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use eyre::{Result, bail};
use indicatif::ProgressBar;

use instar_workspace::{Access, Purpose, Workspace};

use super::{
    arguments::{Arguments, Command, Options},
    checking, formatting, linting,
    reporting::{self, Batch},
    watching::{Change, Watcher},
    workers,
};

pub(super) struct Project {
    pub(super) workspace: Workspace,
    pub(super) selected: BTreeMap<String, PathBuf>,
    pub(super) stdin_module: Option<String>,
    pub(super) stdin: Option<(PathBuf, Arc<str>)>,
}

pub(super) enum Response {
    Completed(Batch),
    Reload,
}

pub(super) fn run(arguments: &Arguments) -> Result<u8> {
    let options = arguments.command.options();

    let root = options
        .project_root
        .clone()
        .map_or_else(env::current_dir, Ok)?
        .canonicalize()?;

    if !root.is_dir() {
        bail!("{}: expected a project directory", root.display());
    }

    let stdin_count = options
        .inputs
        .iter()
        .filter(|path| path.as_os_str() == "-")
        .count();

    if stdin_count > 1 {
        bail!("stdin (-) may only be selected once");
    }

    if stdin_count == 0 && arguments.stdin_filepath.is_some() {
        bail!("--stdin-filepath requires stdin (-) as an input");
    }

    if options.watch && stdin_count != 0 {
        bail!("--watch cannot be used with stdin (-)");
    }

    let overrides = arguments.command.overrides()?;

    let stdin = if stdin_count == 1 {
        Some((
            arguments
                .stdin_filepath
                .clone()
                .unwrap_or_else(|| PathBuf::from("stdin.luau")),
            Arc::from(io::read_to_string(io::stdin().lock())?),
        ))
    } else {
        None
    };

    let mut watcher = options
        .watch
        .then(|| Watcher::new(&root, options, arguments.quiet))
        .transpose()?;

    loop {
        let started = Instant::now();

        let result = prepare(arguments, &root, &overrides, stdin.as_ref()).and_then(|project| {
            match &arguments.command {
                Command::Check(_) => checking::run(arguments, &project, watcher.as_mut(), started),

                Command::Format(format) => workers::run(
                    arguments,
                    &project,
                    &overrides,
                    format.jobs,
                    formatting::run,
                    |pool| {
                        session(
                            arguments,
                            &project,
                            watcher.as_mut(),
                            started,
                            |dirty, progress| pool.execute(dirty, progress),
                        )
                    },
                ),

                Command::Lint(lint) => {
                    let environments = linting::prepare(&project);

                    workers::run(
                        arguments,
                        &project,
                        &overrides,
                        lint.jobs,
                        |workspace, worker| linting::syntax(workspace, worker, &environments),
                        |pool| linting::run(arguments, &project, watcher.as_mut(), started, pool),
                    )
                }
            }
        });

        let Some(watcher) = &mut watcher else {
            return result;
        };

        if let Err(error) = result {
            reporting::failure(&error);
            watcher.wait()?;
        }
    }
}

pub(super) fn session(
    arguments: &Arguments,
    project: &Project,
    mut watcher: Option<&mut Watcher>,
    mut started: Instant,
    mut execute: impl FnMut(&BTreeSet<String>, &ProgressBar) -> Result<Response>,
) -> Result<u8> {
    let mut dirty = BTreeSet::new();

    if let Some(watcher) = &mut watcher {
        watcher.begin(project.workspace.dependencies())?;
    }

    loop {
        tracing::debug!(files = project.selected.len(), "Processing batch");

        let progress = reporting::progress(arguments, &project.workspace, &project.selected)?;
        let result = execute(&dirty, &progress);
        progress.finish_and_clear();

        let Response::Completed(mut batch) = result? else {
            return Ok(0);
        };

        if let Some(watcher) = &mut watcher {
            batch.dependencies.extend(project.workspace.dependencies());
            watcher.observe(mem::take(&mut batch.dependencies))?;
        }

        let summary = batch.report(arguments, project)?;

        summary.report(arguments, project.selected.len(), started);
        let status = summary.status(&arguments.command);

        let Some(watcher) = &mut watcher else {
            return Ok(status);
        };

        match watcher.changed(&project.workspace)? {
            Change::Reload => return Ok(status),

            Change::Sources(changed) => {
                dirty = changed;

                for module in &dirty {
                    project.workspace.invalidate(module);
                }
            }
        }

        started = Instant::now();
    }
}

pub(super) fn open(
    root: &Path,
    options: &Options,
    overrides: &[String],
    stdin: Option<&(PathBuf, Arc<str>)>,
) -> Result<(Workspace, Option<String>)> {
    let mut workspace = Workspace::new(root, &options.configuration, overrides)?;

    let module = stdin
        .map(|(path, source)| workspace.set_source(path, Arc::clone(source)))
        .transpose()?;

    Ok((workspace, module))
}

fn prepare(
    arguments: &Arguments,
    root: &Path,
    overrides: &[String],
    stdin: Option<&(PathBuf, Arc<str>)>,
) -> Result<Project> {
    let options = arguments.command.options();
    let (workspace, stdin_module) = open(root, options, overrides, stdin)?;

    let inputs: Vec<_> = options
        .inputs
        .iter()
        .filter(|path| path.as_os_str() != "-")
        .cloned()
        .collect();

    let (purpose, access) = match &arguments.command {
        Command::Check(_) => (Purpose::Check, Access::Read),
        Command::Format(_) => (Purpose::Format, Access::Write),

        Command::Lint(lint) => (
            Purpose::Lint,
            if lint.fix || lint.fix_preview {
                Access::Write
            } else {
                Access::Read
            },
        ),
    };

    let mut selected = if stdin.is_some() && inputs.is_empty() {
        BTreeMap::new()
    } else {
        workspace.select(&inputs, purpose, access)?
    };

    if let Some(module) = &stdin_module {
        selected.insert(module.clone(), PathBuf::from(module));
    }

    Ok(Project {
        workspace,
        selected,
        stdin_module,
        stdin: stdin.cloned(),
    })
}
