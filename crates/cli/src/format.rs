use std::time::Instant;

use codespan_reporting::diagnostic::{Diagnostic, Label};
use eyre::{Result, eyre};

use instar_core::{
    format::{self, Kind},
    workspace::Workspace,
};

use super::{
    arguments::{Arguments, Operation},
    execution::{Outcome, Project},
    report::{Batch, Output, Source},
    watch::Watcher,
    workers,
};

pub(super) fn session(
    arguments: &Arguments,
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    workers::session(
        arguments,
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
            .ok_or_else(|| eyre!("cannot read source `{module}`"))?,
    };

    let configuration = workspace.formatting(module)?;
    let mut batch = Batch::default();

    match format::format(&source.text, &configuration) {
        Ok(text) => batch.outputs.push(Output { source, text }),

        Err(error) => {
            let diagnostic = match error.kind {
                Kind::Input => Diagnostic::error(),
                Kind::Internal => Diagnostic::bug(),
            };

            batch.findings.push(
                diagnostic
                    .with_code("format")
                    .with_message(error.message)
                    .with_labels(vec![Label::primary(source, error.range)]),
            );
        }
    }

    Ok(batch)
}
