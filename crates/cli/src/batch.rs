use std::{
    collections::BTreeSet,
    io::{self, Write as _},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use eyre::{Report, Result, WrapErr as _};
use similar::TextDiff;

use super::{arguments::Operation, execution::Project, interrupt, output};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Source {
    pub(super) module: String,
    pub(super) text: Arc<str>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Severity {
    Internal,
    Error,
    Warning,
    Information,
    Hint,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Label {
    pub(super) source: Source,
    pub(super) range: Range<usize>,
    pub(super) message: Option<String>,
    pub(super) primary: bool,
}

impl Label {
    pub(super) fn new(source: Source, range: Range<usize>) -> Self {
        Self {
            source,
            range,
            message: None,
            primary: true,
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Finding {
    pub(super) severity: Severity,
    pub(super) code: String,
    pub(super) message: String,
    pub(super) labels: Vec<Label>,
}

impl Finding {
    pub(super) fn new(
        severity: Severity,
        code: impl Into<String>,
        message: String,
        labels: Vec<Label>,
    ) -> Self {
        Self {
            severity,
            code: code.into(),
            message,
            labels,
        }
    }
}

pub(super) struct Output {
    pub(super) source: Source,
    pub(super) text: String,
}

#[derive(Default)]
pub(super) struct Batch {
    pub(super) findings: Vec<Finding>,
    pub(super) outputs: Vec<Output>,
    pub(super) dependencies: BTreeSet<PathBuf>,
    pub(super) reload_dependencies: BTreeSet<PathBuf>,
    failures: Vec<Report>,
}

impl Batch {
    pub(super) fn collect(
        modules: &[String],
        mut process: impl FnMut(&str) -> Result<Self>,
    ) -> Result<Self> {
        let mut batch = Self::default();

        for module in modules {
            interrupt::check()?;

            match process(module) {
                Ok(completed) => batch.append(completed),
                Err(error) => batch.failures.push(error),
            }

            interrupt::check()?;
        }

        Ok(batch)
    }

    pub(super) fn append(&mut self, batch: Self) {
        self.findings.extend(batch.findings);
        self.failures.extend(batch.failures);
        self.outputs.extend(batch.outputs);
        self.dependencies.extend(batch.dependencies);
        self.reload_dependencies.extend(batch.reload_dependencies);
    }

    pub(super) fn publish(mut self, operation: &Operation, project: &Project) -> Result<Summary> {
        self.findings.sort_unstable_by(|left, right| {
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

        self.findings.dedup();

        let mut summary = Summary {
            failures: self.failures.len(),
            ..Summary::default()
        };

        for finding in &self.findings {
            match finding.severity {
                Severity::Internal => summary.failures += 1,
                Severity::Error => summary.errors += 1,
                Severity::Warning => summary.warnings += 1,
                Severity::Information | Severity::Hint => {}
            }
        }

        output::findings(project.workspace.directory(), self.findings)?;

        for error in &self.failures {
            output::failure(error);
        }

        match operation {
            Operation::Format(format) => publish(
                format.check,
                format.preview,
                project,
                self.outputs,
                &mut summary,
            )?,

            Operation::Lint(lint) if lint.corrections => {
                publish(false, lint.preview, project, self.outputs, &mut summary)?;
            }

            Operation::Check(_) | Operation::Lint(_) => {}
        }

        Ok(summary)
    }
}

#[derive(Default)]
pub(super) struct Summary {
    pub(super) errors: usize,
    pub(super) failures: usize,
    pub(super) warnings: usize,
    pub(super) changed: usize,
}

impl Summary {
    pub(super) fn status(&self, operation: &Operation) -> u8 {
        if self.failures != 0 {
            2
        } else {
            u8::from(
                self.errors != 0
                    || matches!(operation, Operation::Format(format) if format.check && self.changed != 0),
            )
        }
    }
}

fn publish(
    check: bool,
    preview: bool,
    project: &Project,
    outputs: Vec<Output>,
    summary: &mut Summary,
) -> Result<()> {
    let writing = if check || preview {
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

        let path = output::relative(
            Path::new(&proposed.source.module),
            project.workspace.directory(),
        );

        let different = proposed.text != proposed.source.text.as_ref();
        summary.changed += usize::from(different);

        if preview && different {
            let name = path.display().to_string();

            write!(
                output,
                "{}",
                TextDiff::from_lines(proposed.source.text.as_ref(), &proposed.text)
                    .unified_diff()
                    .header(&name, &name)
            )
            .wrap_err("Could not write the preview to stdout.")?;
        } else if check {
            if different {
                output::unformatted(&path);
            }
        } else if let Some(writing) = &writing {
            if project.stdin_module.as_ref() == Some(&proposed.source.module) {
                writing
                    .process(&proposed.source.module, || {
                        output.write_all(proposed.text.as_bytes())
                    })
                    .wrap_err("Could not write source text to stdout.")?;
            } else if different
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
