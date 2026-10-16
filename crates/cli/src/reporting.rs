use std::{
    collections::{BTreeMap, BTreeSet},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use clap::ColorChoice;

use codespan_reporting::{
    diagnostic::{Diagnostic, Label, Severity},
    files::SimpleFiles,
    term::{
        self, Config, Styles, StylesWriter,
        termcolor::{ColorChoice as TerminalColor, StandardStream},
    },
};

use color_eyre::config::{HookBuilder, Theme};
use eyre::{Report, Result};
use indicatif::{ProgressBar, ProgressStyle};
use instar_workspace::Workspace;

use super::{arguments::Arguments, execution::Project};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Source {
    pub(super) module: String,
    pub(super) text: Arc<str>,
}

pub(super) type Finding = Diagnostic<Source>;

#[derive(Default)]
pub(super) struct Batch {
    pub(super) findings: Vec<Finding>,
    failures: Vec<Report>,
    pub(super) dependencies: BTreeSet<PathBuf>,
}

#[derive(Default)]
pub(super) struct Summary {
    errors: usize,
    failures: usize,
}

impl Batch {
    pub(super) fn collect(
        modules: &[String],
        progress: &ProgressBar,
        mut process: impl FnMut(&str) -> Result<Self>,
    ) -> Self {
        let mut batch = Self::default();

        for module in modules {
            match process(module) {
                Ok(completed) => batch.append(completed),
                Err(error) => batch.failures.push(error),
            }

            progress.inc(1);
        }

        batch
    }

    pub(super) fn append(&mut self, batch: Self) {
        self.findings.extend(batch.findings);
        self.failures.extend(batch.failures);
        self.dependencies.extend(batch.dependencies);
    }

    pub(super) fn report(self, project: &Project) -> Result<Summary> {
        let mut summary = render(&project.selected, self.findings)?;
        summary.failures += self.failures.len();

        for error in &self.failures {
            failure(error);
        }

        Ok(summary)
    }
}

impl Summary {
    pub(super) fn status(&self) -> u8 {
        if self.failures != 0 {
            2
        } else {
            u8::from(self.errors != 0)
        }
    }

    pub(super) fn report(
        &self,
        arguments: &Arguments,
        files: usize,
        started: Instant,
        jobs: usize,
    ) {
        if arguments.quiet {
            return;
        }

        let errors = self.errors + self.failures;

        eprintln!(
            "{} {files} files; {errors} errors",
            console::style("Checked").for_stderr().bold()
        );

        if arguments.verbose {
            eprintln!("Finished in {:.2?}; worker limit {jobs}", started.elapsed());
        }
    }
}

pub(super) fn initialize(arguments: &Arguments) -> Result<()> {
    match arguments.color.unwrap_or_default() {
        ColorChoice::Always => console::set_colors_enabled_stderr(true),
        ColorChoice::Never => console::set_colors_enabled_stderr(false),
        ColorChoice::Auto => {}
    }

    let builder = HookBuilder::default();

    let builder = if console::colors_enabled_stderr() {
        builder
    } else {
        builder.theme(Theme::new())
    };

    builder.install()?;

    if arguments.verbose {
        tracing_subscriber::fmt()
            .without_time()
            .with_target(false)
            .with_writer(io::stderr)
            .with_ansi(console::colors_enabled_stderr())
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .map_err(Report::msg)?;
    }

    Ok(())
}

pub(super) fn progress(
    arguments: &Arguments,
    workspace: &Workspace,
    selected: &BTreeMap<String, PathBuf>,
) -> Result<ProgressBar> {
    let action = "Checking";

    if arguments.verbose {
        eprintln!("Project: {}", workspace.directory().display());

        for module in selected.keys() {
            eprintln!("{action} {}", path(module, selected).display());
        }
    }

    let progress = if arguments.quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(selected.len().try_into()?)
    };

    let progress = progress.with_style(ProgressStyle::with_template(
        "{spinner} {prefix} {pos}/{len}",
    )?);

    progress.set_prefix(console::style(action).for_stderr().bold().to_string());

    if !progress.is_hidden() {
        progress.enable_steady_tick(Duration::from_millis(100));
    }

    Ok(progress)
}

pub(super) fn failure(error: &Report) {
    eprintln!("{error:?}");
}

fn render(inputs: &BTreeMap<String, PathBuf>, mut findings: Vec<Finding>) -> Result<Summary> {
    findings.sort_unstable_by(|left, right| {
        let position = |finding: &Finding| {
            finding
                .labels
                .first()
                .map(|label| (label.range.start, label.range.end))
        };

        left.labels
            .first()
            .map(|label| &label.file_id)
            .cmp(&right.labels.first().map(|label| &label.file_id))
            .then_with(|| position(left).cmp(&position(right)))
            .then_with(|| left.code.cmp(&right.code))
            .then_with(|| left.message.cmp(&right.message))
    });

    findings.dedup();
    let mut summary = Summary::default();
    let mut files = SimpleFiles::new();
    let mut identities = BTreeMap::new();

    let color = if console::colors_enabled_stderr() {
        TerminalColor::Always
    } else {
        TerminalColor::Never
    };

    let output = StandardStream::stderr(color);
    let styles = Styles::default();
    let mut writer = StylesWriter::new(output.lock(), &styles);

    for finding in findings {
        match finding.severity {
            Severity::Bug => summary.failures += 1,
            Severity::Error => summary.errors += 1,
            Severity::Warning | Severity::Note | Severity::Help => {}
        }

        let labels = finding
            .labels
            .into_iter()
            .map(|label| {
                let identity = *identities.entry(label.file_id.clone()).or_insert_with(|| {
                    files.add(
                        path(&label.file_id.module, inputs).display().to_string(),
                        label.file_id.text,
                    )
                });

                Label {
                    style: label.style,
                    file_id: identity,
                    range: label.range,
                    message: label.message,
                }
            })
            .collect();

        let diagnostic = Diagnostic {
            severity: finding.severity,
            code: finding.code,
            message: finding.message,
            labels,
            notes: finding.notes,
        };

        term::emit_to_write_style(&mut writer, &Config::default(), &files, &diagnostic)?;
    }

    Ok(summary)
}

fn path(module: &str, inputs: &BTreeMap<String, PathBuf>) -> PathBuf {
    let path = Path::new(module);

    let input = inputs.get(module).or_else(|| {
        inputs.values().max_by_key(|input| {
            (
                path.starts_with(input),
                path.components()
                    .zip(input.components())
                    .take_while(|(left, right)| left == right)
                    .count(),
            )
        })
    });

    input.map_or_else(
        || path.to_owned(),
        |input| relative(path, input.parent().unwrap_or(input)),
    )
}

fn relative(path: &Path, root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();

    for ancestor in root.ancestors() {
        if let Ok(path) = path.strip_prefix(ancestor) {
            return prefix.join(path);
        }

        prefix.push("..");
    }

    path.to_owned()
}
