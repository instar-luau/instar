use std::{collections::BTreeMap, sync::Arc, time::Instant};

use eyre::{Result, eyre};

use instar_core::{
    lint,
    settings::lint::Severity,
    workspace::{Error, Origin, SymbolOutcome, Workspace},
};

use super::{
    arguments::Operation,
    batch::{Batch, Finding, Label, Output, Severity as FindingSeverity, Source},
    execution::{Outcome, Project},
    watch::Watcher,
    workers,
};

type Environments = BTreeMap<String, Result<SymbolOutcome, Arc<Error>>>;

pub(super) fn session(
    operation: &Operation,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
) -> Result<Outcome> {
    let environments = prepare(project);
    let corrections = matches!(operation, Operation::Lint(lint) if lint.corrections);

    workers::session(
        operation,
        project,
        watcher,
        started,
        |workspace, modules, progress| {
            let mut pending = Vec::new();

            let batch = Batch::collect(modules, |module| {
                progress.process(module, || {
                    process(workspace, module, &environments, &mut pending)
                })
            })?;

            complete(batch, pending, corrections)
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

fn process(
    workspace: &Workspace,
    module: &str,
    environments: &Environments,
    pending: &mut Vec<(Source, Vec<lint::Finding>)>,
) -> Result<Batch> {
    workspace.invalidate(module);

    let source = Source {
        module: module.to_owned(),
        text: workspace
            .source(module)?
            .ok_or_else(|| eyre!("Could not read source `{module}`."))?,
    };

    let configuration = workspace.linting(module)?;
    let mut batch = Batch::default();

    let symbols = match environments
        .get(module)
        .ok_or_else(|| eyre!("Could not load the lint environment for `{module}`."))?
    {
        Ok(SymbolOutcome::Ready(symbols)) => symbols,

        Ok(SymbolOutcome::Invalid(invalid)) => {
            let module = match &invalid.origin {
                Origin::File(path) => path
                    .to_str()
                    .ok_or_else(|| eyre!("The declaration path must be valid UTF-8."))?,

                Origin::Builtins => "<builtins>",
            };

            batch.findings.push(Finding::new(
                FindingSeverity::Error,
                "syntax",
                invalid.error.message.clone(),
                vec![Label::new(
                    Source {
                        module: module.to_owned(),
                        text: Arc::clone(&invalid.text),
                    },
                    invalid.error.range.clone(),
                )],
            ));

            return Ok(batch);
        }

        Err(error) => return Err(Arc::clone(error).into()),
    };

    match lint::lint(&source.text, &configuration, symbols) {
        Ok(findings) => pending.push((source, findings)),

        Err(failure) => {
            let (code, error) = match failure {
                lint::Failure::Syntax(error) => ("syntax", error),
                lint::Failure::Directive(error) => ("lint", error),
            };

            batch.findings.push(Finding::new(
                FindingSeverity::Error,
                code,
                error.message,
                vec![Label::new(source, error.range)],
            ));
        }
    }

    Ok(batch)
}

fn complete(
    mut batch: Batch,
    pending: Vec<(Source, Vec<lint::Finding>)>,
    corrections: bool,
) -> Result<Batch> {
    for (source, findings) in pending {
        let mut completed = Batch::default();

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
                let severity = match finding.severity {
                    Severity::Error => FindingSeverity::Error,
                    Severity::Warning => FindingSeverity::Warning,
                    Severity::Information => FindingSeverity::Information,
                    Severity::Hint => FindingSeverity::Hint,
                };

                Finding::new(
                    severity,
                    finding.rule,
                    finding.message,
                    vec![Label::new(
                        Source {
                            module: source.module.clone(),
                            text: Arc::clone(&source.text),
                        },
                        finding.range,
                    )],
                )
            }));

        batch.append(completed);
    }

    Ok(batch)
}
