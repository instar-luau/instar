use std::time::Instant;

use codespan_reporting::diagnostic::{Diagnostic, Label};
use eyre::{Result, eyre};

use instar_core::{
    format::{self, Kind},
    workspace::Workspace,
};

use super::{
    arguments::Arguments,
    execution::{self, Project},
    reporting::{Batch, Output, Source},
    watching::Watcher,
    workers,
};

pub(super) fn run(
    arguments: &Arguments,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<u8> {
    workers::run(
        arguments,
        project,
        |workspace, worker| {
            worker.run(workspace, |modules, progress| {
                Ok(Batch::collect(modules, progress, |module| {
                    process(workspace, module)
                }))
            })
        },
        |pool| {
            execution::session(arguments, project, watcher, started, |dirty, progress| {
                pool.execute(dirty, progress)
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
