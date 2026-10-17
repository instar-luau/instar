use std::{collections::BTreeMap, mem, sync::Arc, time::Instant};

use codespan_reporting::diagnostic::{Diagnostic, Label};
use eyre::{Result, eyre};

use instar_core::{
    lint::{self, Symbols},
    settings::lint::Severity,
    workspace::{Error, Workspace},
};

use super::{
    arguments::{Arguments, Operation},
    execution::{self, Outcome, Project, Response},
    report::{Batch, Output, Source},
    watch::Watcher,
    workers,
};

type Environments = BTreeMap<String, Result<Arc<Symbols>, Arc<Error>>>;

pub(super) fn run(
    arguments: &Arguments,
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    let environments = prepare(project);
    let corrections = matches!(operation, Operation::Lint(lint) if lint.fix);

    workers::run(
        arguments,
        operation,
        project,
        |workspace, worker| {
            worker.run(workspace, |modules, progress| {
                Batch::collect(modules, |module| {
                    progress.process(module, || process(workspace, module, &environments))
                })
            })
        },
        |pool| {
            execution::session(
                arguments,
                operation,
                project,
                watcher,
                started,
                |selected, dirty, _, progress| {
                    if selected != &project.selected {
                        return Ok(Response::Reload);
                    }

                    let Response::Completed(batch) = pool.execute(dirty, progress)? else {
                        return Ok(Response::Reload);
                    };

                    Ok(Response::Completed(complete(batch, corrections)?))
                },
            )
        },
    )
}

fn prepare(project: &Project) -> Environments {
    project
        .selected
        .keys()
        .map(|module| {
            (
                module.clone(),
                project.workspace.symbols(module).map_err(Arc::new),
            )
        })
        .collect()
}

fn process(workspace: &Workspace, module: &str, environments: &Environments) -> Result<Batch> {
    workspace.invalidate(module);

    let source = Source {
        module: module.to_owned(),
        text: workspace
            .source(module)?
            .ok_or_else(|| eyre!("cannot read source `{module}`"))?,
    };

    let settings = workspace.linting(module)?;
    let mut batch = Batch::default();

    let symbols = match environments
        .get(module)
        .ok_or_else(|| eyre!("cannot load lint environment for `{module}`"))?
    {
        Ok(symbols) => symbols,

        Err(error) => {
            let Error::Syntax {
                path,
                source: error,
            } = error.as_ref()
            else {
                return Err(Arc::clone(error).into());
            };

            let module = path
                .to_str()
                .ok_or_else(|| eyre!("declaration path must be UTF-8"))?;

            let text = workspace
                .source(module)?
                .ok_or_else(|| eyre!("cannot read declaration source `{module}`"))?;

            batch.findings.push(
                Diagnostic::error()
                    .with_code("syntax")
                    .with_message(error.message.clone())
                    .with_labels(vec![Label::primary(
                        Source {
                            module: module.to_owned(),
                            text,
                        },
                        error.range.clone(),
                    )]),
            );

            return Ok(batch);
        }
    };

    match lint::lint(&source.text, &settings, symbols) {
        Ok(findings) => batch.lint.push((source, findings)),

        Err(error) => batch.findings.push(
            Diagnostic::error()
                .with_code("syntax")
                .with_message(error.message)
                .with_labels(vec![Label::primary(source, error.range)]),
        ),
    }

    Ok(batch)
}

fn complete(mut batch: Batch, corrections: bool) -> Result<Batch> {
    for (source, mut findings) in mem::take(&mut batch.lint) {
        let mut completed = Batch::default();

        if let Err(error) = lint::suppress(&source.text, &mut findings) {
            completed.findings.push(
                Diagnostic::error()
                    .with_code("lint")
                    .with_message(error.message)
                    .with_labels(vec![Label::primary(source, error.range)]),
            );

            batch.append(completed);
            continue;
        }

        if corrections {
            let text = lint::apply(&source.text, &findings)?;

            completed.outputs.push(Output {
                source: source.clone(),
                text,
            });
        }

        completed
            .findings
            .extend(findings.into_iter().map(|finding| {
                let diagnostic = match finding.severity {
                    Severity::Error => Diagnostic::error(),
                    Severity::Warning => Diagnostic::warning(),
                    Severity::Information => Diagnostic::note(),
                    Severity::Hint => Diagnostic::help(),
                };

                diagnostic
                    .with_code(finding.rule)
                    .with_message(finding.message)
                    .with_labels(vec![Label::primary(
                        Source {
                            module: source.module.clone(),
                            text: Arc::clone(&source.text),
                        },
                        finding.range,
                    )])
            }));

        batch.append(completed);
    }

    Ok(batch)
}
