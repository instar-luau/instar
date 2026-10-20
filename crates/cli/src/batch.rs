use std::{collections::BTreeSet, ops::Range, path::PathBuf, sync::Arc};

use eyre::{Report, Result};

use super::interrupt;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Source {
    pub(super) module: String,
    pub(super) text: Arc<str>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
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
    pub(super) is_primary: bool,
}

impl Label {
    pub(super) fn new(source: Source, range: Range<usize>) -> Self {
        Self {
            source,
            range,
            message: None,
            is_primary: true,
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
    pub(super) failures: Vec<Report>,
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
}
