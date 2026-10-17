use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write as _},
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

use color_eyre::{
    Handler,
    config::{HookBuilder, Theme},
};

use eyre::{Report, Result, WrapErr as _};
use indicatif::{ProgressBar, ProgressStyle};

use instar_core::{
    lint,
    workspace::{Error as WorkspaceError, Workspace},
};

use similar::TextDiff;

use super::{
    arguments::{Arguments, Command},
    execution::Project,
};

#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct Source {
    pub(super) module: String,
    pub(super) text: Arc<str>,
}

pub(super) type Finding = Diagnostic<Source>;

pub(super) struct Output {
    pub(super) source: Source,
    pub(super) text: String,
}

#[derive(Default)]
pub(super) struct Batch {
    pub(super) findings: Vec<Finding>,
    pub(super) outputs: Vec<Output>,
    pub(super) lint: Vec<(Source, Vec<lint::Finding>)>,
    pub(super) dependencies: BTreeSet<PathBuf>,
    failures: Vec<Report>,
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
        self.outputs.extend(batch.outputs);
        self.lint.extend(batch.lint);
        self.dependencies.extend(batch.dependencies);
    }

    pub(super) fn report(self, arguments: &Arguments, project: &Project) -> Result<Summary> {
        let mut summary = render(&project.selected, self.findings)?;
        summary.failures += self.failures.len();

        for error in &self.failures {
            failure(error);
        }

        match &arguments.command {
            Command::Format(format) => publish(
                format.check,
                format.preview,
                project,
                self.outputs,
                &mut summary,
            )?,

            Command::Lint(lint) if lint.fix => {
                publish(false, lint.preview, project, self.outputs, &mut summary)?;
            }

            Command::Check(_) | Command::Lint(_) => {}
        }

        Ok(summary)
    }
}

#[derive(Default)]
pub(super) struct Summary {
    errors: usize,
    failures: usize,
    warnings: usize,
    changed: usize,
}

impl Summary {
    pub(super) fn status(&self, command: &Command) -> u8 {
        if self.failures != 0 {
            2
        } else {
            u8::from(
                self.errors != 0
                    || matches!(command, Command::Format(format) if format.check && self.changed != 0),
            )
        }
    }

    pub(super) fn report(&self, arguments: &Arguments, files: usize, started: Instant) {
        if arguments.quiet {
            return;
        }

        let errors = self.errors + self.failures;

        match &arguments.command {
            Command::Check(_) => eprintln!(
                "{} {files} files; {errors} errors",
                console::style("checked").for_stderr().bold()
            ),

            Command::Format(format) => {
                let action = if format.check || format.preview {
                    "inspected"
                } else {
                    "formatted"
                };

                eprintln!(
                    "{} {files} files; {} changed; {errors} errors",
                    console::style(action).for_stderr().bold(),
                    self.changed
                );
            }

            Command::Lint(_) => eprintln!(
                "{} {files} files; {} changed; {errors} errors; {} warnings",
                console::style("linted").for_stderr().bold(),
                self.changed,
                self.warnings,
            ),
        }

        if arguments.verbose {
            eprintln!("finished in {:.2?}", started.elapsed());
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

    builder
        .install()
        .wrap_err("cannot initialize error reporting")?;

    if arguments.verbose {
        tracing_subscriber::fmt()
            .without_time()
            .with_target(false)
            .with_writer(io::stderr)
            .with_ansi(console::colors_enabled_stderr())
            .with_max_level(tracing::Level::DEBUG)
            .try_init()
            .map_err(Report::msg)
            .wrap_err("cannot initialize logging")?;
    }

    Ok(())
}

pub(super) fn progress(
    arguments: &Arguments,
    workspace: &Workspace,
    selected: &BTreeMap<String, PathBuf>,
) -> Result<ProgressBar> {
    let action = match arguments.command {
        Command::Check(_) => "checking",
        Command::Format(_) => "formatting",
        Command::Lint(_) => "linting",
    };

    if arguments.verbose {
        eprintln!("root: `{}`", workspace.directory().display());

        for module in selected.keys() {
            eprintln!("{action} `{}`", path(module, selected).display());
        }
    }

    let progress = if arguments.quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new(
            selected
                .len()
                .try_into()
                .wrap_err("cannot count selected files")?,
        )
    };

    let progress = progress.with_style(
        ProgressStyle::with_template("{spinner} {prefix} {pos}/{len}")
            .wrap_err("cannot configure progress display")?,
    );

    progress.set_prefix(console::style(action).for_stderr().bold().to_string());

    if !progress.is_hidden() {
        progress.enable_steady_tick(Duration::from_millis(100));
    }

    Ok(progress)
}

pub(super) fn failure(error: &Report) {
    let mut notes: Vec<_> = error
        .chain()
        .skip(1)
        .map(|cause| format!("caused by: {cause}"))
        .collect();

    for cause in error.chain() {
        if let Some(WorkspaceError::Configuration(configuration)) =
            cause.downcast_ref::<WorkspaceError>()
            && !configuration.path.is_empty()
            && (configuration.profile.is_none() || configuration.metadata.is_none())
        {
            notes.push(format!(
                "configuration key: `{}`",
                configuration.path.join(".")
            ));
        }
    }

    if let Some(handler) = error.handler().downcast_ref::<Handler>() {
        if let Some(trace) = handler
            .span_trace()
            .map(ToString::to_string)
            .filter(|trace| !trace.is_empty())
        {
            notes.push(format!("span trace:\n{trace}"));
        }

        if let Some(trace) = handler.backtrace() {
            notes.push(format!("backtrace:\n{trace:?}"));
        }
    }

    let diagnostic = Diagnostic::error()
        .with_message(error.to_string())
        .with_notes(notes);

    if let Err(source) = render(&BTreeMap::new(), vec![diagnostic]) {
        eprintln!("error: {error:#}\nerror: {source:#}");
    }
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
            Severity::Warning => summary.warnings += 1,
            Severity::Note | Severity::Help => {}
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

        term::emit_to_write_style(&mut writer, &Config::default(), &files, &diagnostic)
            .wrap_err("cannot render diagnostic")?;
    }

    Ok(summary)
}

fn publish(
    check: bool,
    preview: bool,
    project: &Project,
    outputs: Vec<Output>,
    summary: &mut Summary,
) -> Result<()> {
    let mut output = io::stdout().lock();

    for proposed in outputs {
        let path = path(&proposed.source.module, &project.selected);
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
            .wrap_err("cannot write preview to stdout")?;
        } else if check {
            if different {
                eprintln!("would format `{}`", path.display());
            }
        } else if !preview {
            if project.stdin_module.as_ref() == Some(&proposed.source.module) {
                output
                    .write_all(proposed.text.as_bytes())
                    .wrap_err("cannot write source to stdout")?;
            } else if different
                && let Err(error) = project.workspace.write(
                    &proposed.source.module,
                    &proposed.source.text,
                    &proposed.text,
                )
            {
                summary.failures += 1;
                failure(&Report::new(error));
            }
        }
    }

    output.flush().wrap_err("cannot flush stdout")?;

    Ok(())
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
