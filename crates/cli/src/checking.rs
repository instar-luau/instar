use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::Arc,
};

use codespan_reporting::{
    diagnostic::{Diagnostic, Label},
    files::{Files as _, SimpleFile},
};

use eyre::{Result, eyre};
use instar_luau::{Context, Frontend};
use instar_workspace::Workspace;

use super::{
    reporting::{Batch, Source},
    workers::Worker,
};

pub(super) fn run(workspace: &Workspace, worker: Worker<'_>) -> Result<()> {
    let context = Context::new(workspace);
    let mut frontend = Frontend::new(&context, false)?;

    worker.run(workspace, |modules, dirty, progress| {
        for module in dirty {
            frontend.invalidate(module)?;
        }

        let mut files = BTreeMap::new();

        Ok(Batch::collect(modules, progress, |module| {
            process(workspace, &mut frontend, module, &mut files)
        }))
    })
}

fn process(
    workspace: &Workspace,
    frontend: &mut Frontend<'_>,
    module: &str,
    files: &mut BTreeMap<String, SimpleFile<String, Arc<str>>>,
) -> Result<Batch> {
    let mut batch = Batch::default();

    for diagnostic in frontend.check(module)? {
        let mut finding = Diagnostic::error()
            .with_code(diagnostic.code.to_string())
            .with_message(diagnostic.message);

        if !diagnostic.module.is_empty() {
            let file = match files.entry(diagnostic.module.clone()) {
                Entry::Occupied(entry) => entry.into_mut(),

                Entry::Vacant(entry) => {
                    let source = workspace.snapshot(&diagnostic.module).ok_or_else(|| {
                        eyre!("Diagnostic source is unavailable: {}", diagnostic.module)
                    })?;

                    entry.insert(SimpleFile::new(diagnostic.module.clone(), source))
                }
            };

            let start = file.line_range((), diagnostic.start_line as usize)?.start
                + diagnostic.start_column as usize;

            let end = file.line_range((), diagnostic.end_line as usize)?.start
                + diagnostic.end_column as usize;

            let source = Source {
                module: diagnostic.module,
                text: Arc::clone(file.source()),
            };

            finding.labels.push(Label::primary(source, start..end));
        }

        batch.findings.push(finding);
    }

    Ok(batch)
}
