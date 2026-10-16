use std::{collections::BTreeMap, mem, sync::Arc, time::Instant};

use codespan_reporting::diagnostic::{Diagnostic, Label};
use eyre::{Result, bail, eyre};
use indicatif::ProgressBar;
use instar_configuration::linting::Severity;
use instar_luau::{Context, Frontend};
use instar_workspace::Workspace;

use super::{
    arguments::{Arguments, Command},
    checking,
    execution::{self, Project, Response},
    reporting::{Batch, Output, Source},
    watching::Watcher,
    workers::{Pool, Worker},
};

pub(super) fn syntax(workspace: &Workspace, worker: Worker<'_>) -> Result<()> {
    worker.run(workspace, |modules, _| {
        Ok(Batch::collect(modules, &ProgressBar::hidden(), |module| {
            workspace.invalidate(module);

            let source = Source {
                module: module.to_owned(),
                text: workspace
                    .source(module)?
                    .ok_or_else(|| eyre!("{module}: source is unavailable"))?,
            };

            let settings = workspace.linting(module)?;
            let mut batch = Batch::default();

            match instar_lint::lint(&source.text, &settings) {
                Ok(findings) => batch.lint.push((source, findings)),

                Err(error) => batch.findings.push(
                    Diagnostic::error()
                        .with_code("syntax")
                        .with_message(error.message)
                        .with_labels(vec![Label::primary(source, error.range)]),
                ),
            }

            Ok(batch)
        }))
    })
}

pub(super) fn run(
    arguments: &Arguments,
    project: &Project,
    watcher: Option<&mut Watcher>,
    started: Instant,
    pool: &Pool,
) -> Result<u8> {
    let workspace = &project.workspace;
    let context = Context::new(workspace);
    let mut frontend = Frontend::new(&context, false)?;
    let modules: Vec<_> = project.selected.keys().cloned().collect();

    let corrections =
        matches!(&arguments.command, Command::Lint(lint) if lint.fix || lint.fix_preview);

    execution::session(arguments, project, watcher, started, |dirty, progress| {
        for module in dirty {
            frontend.invalidate(module)?;
        }

        let Response::Completed(mut batch) = pool.execute(dirty, progress)? else {
            return Ok(Response::Reload);
        };

        let mut pending: BTreeMap<_, _> = mem::take(&mut batch.lint)
            .into_iter()
            .map(|(source, findings)| (source.module.clone(), (source, findings)))
            .collect();

        let mut files = BTreeMap::new();

        batch.append(Batch::collect(&modules, progress, |module| {
            let Some((source, mut findings)) = pending.remove(module) else {
                return Ok(Batch::default());
            };

            let analysis = frontend.lint(module)?;

            if workspace.snapshot(module).as_deref() != Some(source.text.as_ref()) {
                bail!("{module}: source changed between lint passes");
            }

            let blocked = analysis
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.blocking);

            let mut completed = checking::diagnostics(
                workspace,
                analysis
                    .diagnostics
                    .into_iter()
                    .filter(|diagnostic| diagnostic.blocking),
                &mut files,
            )?;

            let settings = workspace.linting(module)?;

            let warnings: Vec<_> = analysis
                .warnings
                .into_iter()
                .filter(|warning| warning.diagnostic.module == module)
                .collect();

            findings.extend(instar_lint::analyze(&source.text, &warnings, &settings)?);

            if let Err(error) = instar_lint::suppress(&source.text, &mut findings) {
                completed.findings.push(
                    Diagnostic::error()
                        .with_code("lint")
                        .with_message(error.message)
                        .with_labels(vec![Label::primary(source, error.range)]),
                );

                return Ok(completed);
            }

            if corrections && !blocked {
                let text = instar_lint::apply(&source.text, &findings)?;

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

            Ok(completed)
        }));

        Ok(Response::Completed(batch))
    })
}
