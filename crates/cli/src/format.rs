use std::time::Instant;

use eyre::{Result, eyre};

use instar_core::{
    format::{self, Kind},
    workspace::Workspace,
};

use super::{
    arguments::Operation,
    batch::{Batch, Finding, Label, Output, Severity, Source},
    execution::{Outcome, Project},
    watch::Watcher,
    workers,
};

pub(super) fn session(
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    workers::session(
        operation,
        project,
        watcher,
        started,
        |workspace, modules, progress| {
            Batch::collect(modules, |module| {
                progress.process(module, || process(workspace, module))
            })
        },
    )
}

fn process(workspace: &Workspace, module: &str) -> Result<Batch> {
    workspace.invalidate(module);

    let source = Source {
        module: module.to_owned(),
        text: workspace
            .source(module)?
            .ok_or_else(|| eyre!("Could not read source `{module}`."))?,
    };

    let configuration = workspace.formatting(module)?;
    let mut batch = Batch::default();

    match format::format(&source.text, &configuration) {
        Ok(text) => batch.outputs.push(Output { source, text }),

        Err(error) => {
            let severity = match error.kind {
                Kind::Input => Severity::Error,
                Kind::Internal => Severity::Internal,
            };

            batch.findings.push(Finding::new(
                severity,
                "format",
                error.message,
                vec![Label::new(source, error.range)],
            ));
        }
    }

    Ok(batch)
}
