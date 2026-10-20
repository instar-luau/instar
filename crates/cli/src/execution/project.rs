use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use eyre::{Report, Result};
use instar_core::workspace::{Access, Generators, Purpose, Workspace};

use super::watch::Watcher;

use crate::{
    arguments::{Operation, Options},
    interrupt,
    output::{self, Progress},
};

pub(crate) struct Project {
    pub(crate) workspace: Workspace,
    pub(crate) generators: Option<Generators>,
    pub(crate) overrides: Vec<String>,
    pub(crate) selected: BTreeMap<String, PathBuf>,
    pub(crate) stdin_module: Option<String>,
    pub(crate) stdin: Option<(PathBuf, Arc<str>)>,
    pub(crate) progress: Progress,
}

pub(super) fn open(
    root: &Path,
    options: &Options,
    overrides: &[String],
    stdin: Option<&(PathBuf, Arc<str>)>,
) -> Result<(Workspace, Option<String>)> {
    let workspace = Workspace::new(root, &options.configuration, overrides)?
        .with_cancellation(interrupt::is_requested);

    let module = stdin
        .map(|(path, source)| workspace.set_source(path, Arc::clone(source)))
        .transpose()?;

    Ok((workspace, module))
}

pub(super) fn prepare(
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

    let generators = if matches!(operation, Operation::Format(_)) {
        None
    } else {
        watcher.as_ref().map(|watcher| watcher.generators.clone())
    };

    if let Some(generators) = &generators {
        workspace = workspace.with_generators(generators.clone());
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
        generators,
        overrides: overrides.to_owned(),
        selected,
        stdin_module,
        stdin: stdin.cloned(),
        progress,
    })
}

pub(super) fn shutdown(generators: &Generators) -> Result<()> {
    generators.shutdown().map_err(|error| {
        let error = Report::new(error);

        if interrupt::is_requested() {
            output::failure(&error);
        }

        error
    })
}

pub(super) fn select(
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
            if lint.should_fix {
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
