use codespan_reporting::diagnostic::{Diagnostic, Label};
use eyre::{Result, eyre};
use instar_format::Kind;
use instar_workspace::Workspace;

use super::{
    reporting::{Batch, Output, Source},
    workers::Worker,
};

pub(super) fn run(workspace: &Workspace, worker: Worker<'_>) -> Result<()> {
    worker.run(workspace, |modules, _, progress| {
        Ok(Batch::collect(modules, progress, |module| {
            process(workspace, module)
        }))
    })
}

fn process(workspace: &Workspace, module: &str) -> Result<Batch> {
    workspace.invalidate(module);

    let source = Source {
        module: module.to_owned(),
        text: workspace
            .source(module)?
            .ok_or_else(|| eyre!("{module}: source is unavailable"))?,
    };

    let configuration = workspace.formatting(module)?;
    let mut batch = Batch::default();

    match instar_format::format(&source.text, &configuration) {
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
