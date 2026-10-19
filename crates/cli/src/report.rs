use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
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

use instar_core::{assets::Generated, workspace::Error as WorkspaceError};

use similar::TextDiff;

use super::{
    arguments::{Arguments, Command, Operation},
    execution::Project,
    interrupt,
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

    pub(super) fn report(
        self,
        operation: &Operation,
        project: &Project,
        selected: &BTreeMap<String, PathBuf>,
    ) -> Result<Summary> {
        let mut summary = render(selected, self.findings)?;
        summary.failures += self.failures.len();

        for error in &self.failures {
            failure(error);
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
    errors: usize,
    failures: usize,
    warnings: usize,
    changed: usize,
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

    pub(super) fn report(
        &self,
        arguments: &Arguments,
        operation: &Operation,
        files: usize,
        started: Instant,
    ) {
        if arguments.quiet {
            return;
        }

        let errors = self.errors + self.failures;

        let action = match operation {
            Operation::Check(_) => "checked",
            Operation::Format(format) if format.check || format.preview => "inspected",
            Operation::Format(_) => "formatted",
            Operation::Lint(_) => "linted",
        };

        let action = console::style(action).for_stderr().bold();

        let action = if errors != 0 {
            action.red()
        } else if matches!(operation, Operation::Format(format) if format.check)
            && self.changed != 0
        {
            action.yellow()
        } else {
            action.green()
        };

        let files = format!(
            "{} {} {}",
            console::style(files).for_stderr().cyan().bold(),
            console::style("files in").for_stderr().dim(),
            console::style(format!("{:.2?}", started.elapsed()))
                .for_stderr()
                .cyan()
        );

        let errors = format!(
            "{} {}",
            if errors == 0 {
                console::style(errors).for_stderr().green()
            } else {
                console::style(errors).for_stderr().red().bold()
            },
            console::style("errors").for_stderr().dim()
        );

        let changed = format!(
            "{} {}",
            if self.changed == 0 {
                console::style(self.changed).for_stderr().dim()
            } else {
                console::style(self.changed).for_stderr().yellow().bold()
            },
            console::style("changed").for_stderr().dim()
        );

        match operation {
            Operation::Check(_) => eprintln!("{action} {files}; {errors}"),
            Operation::Format(_) => eprintln!("{action} {files}; {changed}; {errors}"),

            Operation::Lint(_) => eprintln!(
                "{action} {files}; {changed}; {errors}; {} {}",
                if self.warnings == 0 {
                    console::style(self.warnings).for_stderr().dim()
                } else {
                    console::style(self.warnings).for_stderr().yellow().bold()
                },
                console::style("warnings").for_stderr().dim()
            ),
        }
    }
}

pub(super) fn initialize(color: Option<ColorChoice>, verbose: bool) -> Result<()> {
    match color.unwrap_or_default() {
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

    if verbose {
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

pub(super) fn generated(
    arguments: &Arguments,
    generated: &Generated,
    root: &Path,
    association: Option<&Path>,
    started: Instant,
) {
    if arguments.quiet {
        return;
    }

    if arguments.verbose {
        eprintln!(
            "{}: `{}`",
            console::style("root").for_stderr().cyan(),
            console::style(root.display()).for_stderr().magenta()
        );

        for path in &generated.files {
            eprintln!(
                "{} `{}`",
                console::style("generated").for_stderr().cyan().bold(),
                console::style(path.display()).for_stderr().magenta()
            );
        }

        if let Some(path) = association {
            eprintln!(
                "{} `{}`",
                console::style("associated").for_stderr().cyan().bold(),
                console::style(path.display()).for_stderr().magenta()
            );
        }
    }

    eprintln!(
        "{} {} {} {}; {} {}",
        console::style("generated").for_stderr().green().bold(),
        console::style(generated.files.len())
            .for_stderr()
            .cyan()
            .bold(),
        console::style("files in").for_stderr().dim(),
        console::style(format!("{:.2?}", started.elapsed()))
            .for_stderr()
            .cyan(),
        if generated.changed == 0 {
            console::style(generated.changed).for_stderr().dim()
        } else {
            console::style(generated.changed)
                .for_stderr()
                .yellow()
                .bold()
        },
        console::style("changed").for_stderr().dim()
    );
}

pub(super) fn interrupted(arguments: &Arguments) {
    if arguments.quiet {
        return;
    }

    if matches!(
        &arguments.command,
        Command::Files(operation) if operation.options().watch
    ) {
        eprintln!(
            "{} {}",
            console::style("stopped").for_stderr().green().bold(),
            console::style("watching").for_stderr().dim()
        );
    } else {
        let command = match &arguments.command {
            Command::Generate(_) => "generate",

            Command::Files(operation) => match operation.as_ref() {
                Operation::Check(_) => "check",
                Operation::Format(_) => "format",
                Operation::Lint(_) => "lint",
            },
        };

        eprintln!(
            "{} {}",
            console::style("interrupted").for_stderr().yellow().bold(),
            console::style(command).for_stderr().cyan()
        );
    }
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
                eprintln!(
                    "{} `{}`",
                    console::style("would format").for_stderr().yellow(),
                    console::style(path.display()).for_stderr().magenta()
                );
            }
        } else if let Some(writing) = &writing {
            if project.stdin_module.as_ref() == Some(&proposed.source.module) {
                writing
                    .process(&proposed.source.module, || {
                        output.write_all(proposed.text.as_bytes())
                    })
                    .wrap_err("cannot write source to stdout")?;
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
                writing.suspend(|| failure(&Report::new(error)));
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

pub(super) fn relative(path: &Path, root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();

    for ancestor in root.ancestors() {
        if let Ok(path) = path.strip_prefix(ancestor) {
            return prefix.join(path);
        }

        prefix.push("..");
    }

    path.to_owned()
}
