use std::{
    io::{self, Write as _},
    path::Path,
};

use eyre::{Report, Result, WrapErr as _};
use similar::TextDiff;

use super::summary::Summary;

use crate::{
    arguments::Operation,
    batch::{Batch, Finding, Output, Severity},
    execution::Project,
    interrupt, output,
    paths::relative,
};

pub(crate) fn publish(
    mut batch: Batch,
    operation: &Operation,
    project: &Project,
) -> Result<Summary> {
    batch.findings.sort_unstable_by(|left, right| {
        let position = |finding: &Finding| {
            finding
                .labels
                .first()
                .map(|label| (label.range.start, label.range.end))
        };

        left.labels
            .first()
            .map(|label| &label.source)
            .cmp(&right.labels.first().map(|label| &label.source))
            .then_with(|| position(left).cmp(&position(right)))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
    });

    batch.findings.dedup();

    let mut summary = Summary {
        failures: batch.failures.len(),
        ..Summary::default()
    };

    for finding in &batch.findings {
        match finding.severity {
            Severity::Internal => summary.failures += 1,
            Severity::Error => summary.errors += 1,
            Severity::Warning => summary.warnings += 1,
            Severity::Information | Severity::Hint => {}
        }
    }

    output::findings(project.workspace.directory(), batch.findings)?;

    for error in &batch.failures {
        output::failure(error);
    }

    match operation {
        Operation::Format(format) => changes(
            format.should_check,
            format.should_preview,
            project,
            batch.outputs,
            &mut summary,
        )?,

        Operation::Lint(lint) if lint.should_fix => {
            changes(
                false,
                lint.should_preview,
                project,
                batch.outputs,
                &mut summary,
            )?;
        }

        Operation::Check(_) | Operation::Lint(_) => {}
    }

    Ok(summary)
}

fn changes(
    should_check: bool,
    should_preview: bool,
    project: &Project,
    outputs: Vec<Output>,
    summary: &mut Summary,
) -> Result<()> {
    let writing = if should_check || should_preview {
        None
    } else {
        let files = outputs
            .iter()
            .filter(|proposed| {
                project.stdin_module.as_ref() == Some(&proposed.source.module)
                    || proposed.text != proposed.source.text.as_ref()
            })
            .count();

        (files != 0).then(|| project.progress.writing(files))
    };

    let mut output = io::stdout().lock();

    for proposed in outputs {
        interrupt::check()?;

        let path = relative(
            Path::new(&proposed.source.module),
            project.workspace.directory(),
        );

        let is_different = proposed.text != proposed.source.text.as_ref();
        summary.changed += usize::from(is_different);

        if should_preview && is_different {
            let name = path.display().to_string();

            write!(
                output,
                "{}",
                TextDiff::from_lines(proposed.source.text.as_ref(), &proposed.text)
                    .unified_diff()
                    .header(&name, &name)
            )
            .wrap_err("Could not write the preview to stdout.")?;
        } else if should_check {
            if is_different {
                output::unformatted(&path);
            }
        } else if let Some(writing) = &writing {
            if project.stdin_module.as_ref() == Some(&proposed.source.module) {
                writing
                    .process(&proposed.source.module, || {
                        output.write_all(proposed.text.as_bytes())
                    })
                    .wrap_err("Could not write source text to stdout.")?;
            } else if is_different
                && let Err(error) = writing.process(&proposed.source.module, || {
                    project.workspace.write(
                        &proposed.source.module,
                        &proposed.source.text,
                        &proposed.text,
                    )
                })
            {
                summary.failures += 1;
                output::failure(&Report::new(error));
            }
        }
    }

    output.flush().wrap_err("Could not flush stdout.")?;

    Ok(())
}
