use std::{
    collections::{BTreeMap, BTreeSet},
    env, io, mem,
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use eyre::{Result, WrapErr as _, bail};
use indicatif::ProgressBar;
use instar_core::workspace::{Access, Purpose, Workspace};

use super::{
    arguments::{Arguments, Command, Options},
    check, format, lint,
    report::{self, Batch},
    watch::{Change, Watcher},
};

pub(super) struct Project {
    pub(super) workspace: Workspace,
    pub(super) overrides: Vec<String>,
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
        .root
        .clone()
        .map_or_else(env::current_dir, Ok)
        .wrap_err("cannot resolve working directory")?;

    let root = root
        .canonicalize()
        .wrap_err_with(|| format!("cannot resolve root `{}`", root.display()))?;

    if !root.is_dir() {
        bail!("project root `{}` must be a directory", root.display());
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
            Arc::from(io::read_to_string(io::stdin().lock()).wrap_err("cannot read stdin")?),
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
                Command::Check(_) => check::run(arguments, &project, watcher.as_mut(), started),
                Command::Format(_) => format::run(arguments, &project, watcher.as_mut(), started),
                Command::Lint(_) => lint::run(arguments, &project, watcher.as_mut(), started),
            }
        });

        let Some(watcher) = &mut watcher else {
            return result;
        };

        if let Err(error) = result {
            report::failure(&error);
            watcher.ready();
            watcher.wait(true)?;
        }
    }
}

pub(super) fn session(
    arguments: &Arguments,
    project: &Project,
    mut watcher: Option<&mut Watcher>,
    mut started: Instant,
    mut execute: impl FnMut(
        &BTreeMap<String, PathBuf>,
        &BTreeSet<String>,
        bool,
        &ProgressBar,
    ) -> Result<Response>,
) -> Result<u8> {
    let mut dirty = BTreeSet::new();
    let mut selected = project.selected.clone();
    let mut discovery = false;

    if let Some(watcher) = &mut watcher {
        watcher.begin(&project.workspace)?;
    }

    loop {
        tracing::debug!(files = selected.len(), "processing batch");

        let progress = report::progress(arguments, &project.workspace, &selected)?;
        let result = execute(&selected, &dirty, discovery, &progress);
        progress.finish_and_clear();

        let Response::Completed(mut batch) = result? else {
            return Ok(0);
        };

        if let Some(watcher) = &mut watcher {
            batch.dependencies.extend(project.workspace.dependencies());
            watcher.observe(project.workspace.reload_dependencies(), true)?;
            watcher.observe(mem::take(&mut batch.dependencies), false)?;
        }

        let summary = batch.report(arguments, project, &selected)?;

        summary.report(arguments, selected.len(), started);
        let status = summary.status(&arguments.command);

        let Some(watcher) = &mut watcher else {
            return Ok(status);
        };

        watcher.ready();

        loop {
            let Change::Sources {
                paths,
                discovery: structural,
            } = watcher.changed(&project.workspace)?
            else {
                return Ok(status);
            };

            let previous = if structural {
                mem::replace(
                    &mut selected,
                    select(
                        arguments,
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

            tracing::debug!("ignoring filesystem changes outside active inputs");
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
    let selected = select(arguments, &workspace, stdin_module.as_deref())?;

    Ok(Project {
        workspace,
        overrides: overrides.to_owned(),
        selected,
        stdin_module,
        stdin: stdin.cloned(),
    })
}

fn select(
    arguments: &Arguments,
    workspace: &Workspace,
    stdin_module: Option<&str>,
) -> Result<BTreeMap<String, PathBuf>> {
    let options = arguments.command.options();

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
            if lint.fix {
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
