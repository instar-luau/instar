use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use eyre::{Report, Result, WrapErr as _};

use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, MietteDiagnostic, NamedSource,
    SourceCode,
};

use instar_core::{assets::Generated, workspace::Error as WorkspaceError};

use super::delivery::{self, Capture, Mode};

use crate::{
    arguments::{Arguments, Command, Operation},
    batch::{Finding, Severity, Summary},
};

static DETAILS: AtomicBool = AtomicBool::new(false);

#[derive(Debug)]
pub(crate) struct Validation {
    message: &'static str,
    help: &'static str,
}

impl Validation {
    pub(crate) fn new(message: &'static str, help: &'static str) -> Self {
        Self { message, help }
    }
}

impl fmt::Display for Validation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for Validation {}

#[derive(Debug)]
struct View<'error> {
    diagnostic: MietteDiagnostic,
    source: Option<NamedSource<Arc<str>>>,
    cause: Option<&'error (dyn Error + 'static)>,
    related: Vec<Self>,
}

impl View<'_> {
    fn new(diagnostic: MietteDiagnostic) -> Self {
        Self {
            diagnostic,
            source: None,
            cause: None,
            related: Vec::new(),
        }
    }
}

impl fmt::Display for View<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.diagnostic, formatter)
    }
}

impl Error for View<'_> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause
    }
}

impl Diagnostic for View<'_> {
    fn code(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.diagnostic.code()
    }

    fn severity(&self) -> Option<miette::Severity> {
        self.diagnostic.severity()
    }

    fn help(&self) -> Option<Box<dyn fmt::Display + '_>> {
        self.diagnostic.help()
    }

    fn labels(&self) -> Option<Box<dyn Iterator<Item = LabeledSpan> + '_>> {
        self.diagnostic.labels()
    }

    fn source_code(&self) -> Option<&dyn SourceCode> {
        self.source.as_ref().map(|source| source as &dyn SourceCode)
    }

    fn related(&self) -> Option<Box<dyn Iterator<Item = &dyn Diagnostic> + '_>> {
        (!self.related.is_empty()).then(|| {
            Box::new(
                self.related
                    .iter()
                    .map(|related| related as &dyn Diagnostic),
            ) as Box<dyn Iterator<Item = &dyn Diagnostic>>
        })
    }
}

pub(crate) fn summary(summary: &Summary, operation: &Operation, files: usize, started: Instant) {
    if delivery::mode() == Mode::Quiet {
        return;
    }

    let errors = summary.errors + summary.failures;

    let action = match operation {
        Operation::Check(_) => "Checked",
        Operation::Format(format) if format.check => "Checked formatting of",
        Operation::Format(format) if format.preview => "Previewed formatting for",
        Operation::Format(_) => "Formatted",
        Operation::Lint(lint) if lint.preview => "Previewed fixes for",
        Operation::Lint(_) => "Linted",
    };

    let action = console::style(action).for_stderr().bold();

    let action = if errors != 0 {
        action.red()
    } else if summary.warnings != 0
        || (matches!(operation, Operation::Format(format) if format.check) && summary.changed != 0)
    {
        action.yellow()
    } else {
        action.green()
    };

    let mut details = Vec::new();

    if errors != 0 {
        details.push(format!(
            "{errors} {}",
            delivery::plural(errors, "error", "errors")
        ));
    }

    if summary.warnings != 0 {
        details.push(format!(
            "{} {}",
            summary.warnings,
            delivery::plural(summary.warnings, "warning", "warnings"),
        ));
    }

    if summary.changed != 0 {
        let count = summary.changed;
        let word = delivery::plural(count, "file", "files");

        details.push(match operation {
            Operation::Format(format) if format.check => format!(
                "{count} {word} {} formatting",
                delivery::plural(count, "needs", "need"),
            ),

            Operation::Format(format) if format.preview => format!("{count} {word} would change"),
            Operation::Lint(lint) if lint.preview => format!("{count} {word} would change"),

            Operation::Check(_) | Operation::Format(_) | Operation::Lint(_) => {
                format!("{count} {word} changed")
            }
        });
    }

    let detail = if details.is_empty() {
        if files == 0 {
            "No files selected.".to_owned()
        } else {
            match operation {
                Operation::Check(_) => "No errors.",
                Operation::Format(format) if format.check => "All files are formatted.",
                Operation::Format(_) => "No changes needed.",
                Operation::Lint(lint) if lint.preview => "No changes needed.",
                Operation::Lint(_) => "No errors or warnings.",
            }
            .to_owned()
        }
    } else {
        format!("{}.", details.join(delivery::separator()))
    };

    if DETAILS.swap(false, Ordering::Relaxed) {
        delivery::message("");
    }

    delivery::message(format_args!(
        "{action} {files} {} in {:.2}s\n  {detail}",
        delivery::plural(files, "file", "files"),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn generated(
    generated: &Generated,
    root: &Path,
    association: Option<&Path>,
    started: Instant,
) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    let width = if association.is_some() {
        "Associated".len()
    } else {
        "Generated".len()
    } + 2;

    for path in &generated.files {
        delivery::file("Generated", &relative(path, root), width);
    }

    if let Some(path) = association {
        delivery::file("Associated", &relative(path, root), width);
    }

    let detail = if generated.changed == 0 {
        "No changes needed.".to_owned()
    } else {
        format!(
            "{} {} changed.",
            generated.changed,
            delivery::plural(generated.changed, "file", "files"),
        )
    };

    delivery::message(format_args!(
        "{} {} {} in {:.2}s\n  {detail}",
        console::style("Generated").for_stderr().green().bold(),
        generated.files.len(),
        delivery::plural(generated.files.len(), "file", "files"),
        started.elapsed().as_secs_f64(),
    ));
}

pub(crate) fn interrupted(arguments: &Arguments) {
    delivery::progress().finish_and_clear();

    if delivery::mode() == Mode::Quiet {
        return;
    }

    let message = if matches!(
        &arguments.command,
        Command::Files(operation) if operation.options().watch
    ) {
        "Stopped watching."
    } else {
        match &arguments.command {
            Command::Generate(_) => "Generation interrupted.",
            Command::Server => "Server interrupted.",

            Command::Files(operation) => match operation.as_ref() {
                Operation::Check(_) => "Check interrupted.",
                Operation::Format(_) => "Formatting interrupted.",
                Operation::Lint(_) => "Linting interrupted.",
            },
        }
    };

    delivery::message(console::style(message).for_stderr().yellow().bold());
}

pub(crate) fn failure(error: &Report) {
    delivery::progress().finish_and_clear();
    let mut view = View::new(MietteDiagnostic::new(error.to_string()));
    view.cause = error.source();

    for cause in error.chain() {
        if let Some(validation) = cause.downcast_ref::<Validation>() {
            view.diagnostic.help = Some(validation.help.to_owned());
        }

        if let Some(WorkspaceError::Configuration(configuration)) =
            cause.downcast_ref::<WorkspaceError>()
            && !configuration.path.is_empty()
            && (configuration.profile.is_none() || configuration.metadata.is_none())
        {
            view.related.push(note(format!(
                "Configuration key: `{}`",
                configuration.path.join("."),
            )));
        }
    }

    if delivery::mode() == Mode::Debug
        && let Some(capture) = error.handler().downcast_ref::<Capture>()
    {
        view.related.extend(capture.details().into_iter().map(note));
    }

    if let Err(source) = render(&view, Severity::Error) {
        delivery::message(format_args!(
            "error: {error:#}\nerror: Could not render the diagnostic: {source:#}",
        ));
    }

    DETAILS.store(true, Ordering::Relaxed);
}

pub(super) fn panic(message: &str, capture: &Capture) {
    delivery::progress().finish_and_clear();
    let mut view = View::new(MietteDiagnostic::new(message));
    view.related.extend(capture.details().into_iter().map(note));

    if let Err(error) = render(&view, Severity::Error) {
        delivery::message(format_args!(
            "error: {message}\nerror: Could not render the diagnostic: {error:#}",
        ));
    }
}

fn note<'error>(message: String) -> View<'error> {
    View::new(MietteDiagnostic::new(message).with_severity(miette::Severity::Advice))
}

pub(crate) fn findings(root: &Path, findings: Vec<Finding>) -> Result<()> {
    for finding in findings {
        let severity = match finding.severity {
            Severity::Internal | Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
            Severity::Information | Severity::Hint => miette::Severity::Advice,
        };

        let diagnostic = MietteDiagnostic::new(&finding.message)
            .with_code(&finding.code)
            .with_severity(severity);

        let mut sources: BTreeMap<_, Vec<_>> = BTreeMap::new();

        let primary = finding
            .labels
            .iter()
            .find(|label| label.primary)
            .or_else(|| finding.labels.first())
            .map(|label| label.source.clone());

        for label in finding.labels {
            let span = if label.primary {
                LabeledSpan::new_primary_with_span(label.message, label.range)
            } else {
                LabeledSpan::new_with_span(label.message, label.range)
            };

            sources.entry(label.source).or_default().push(span);
        }

        let mut view = View::new(diagnostic.clone());

        if let Some(primary) = primary {
            view.source = Some(NamedSource::new(
                relative(Path::new(&primary.module), root)
                    .display()
                    .to_string(),
                Arc::clone(&primary.text),
            ));

            view.diagnostic.labels = sources.remove(&primary);
        }

        for (source, labels) in sources {
            let mut related = View::new(diagnostic.clone().with_labels(labels));

            related.source = Some(NamedSource::new(
                relative(Path::new(&source.module), root)
                    .display()
                    .to_string(),
                source.text,
            ));

            view.related.push(related);
        }

        if let Err(error) = render(&view, finding.severity) {
            let severity = match finding.severity {
                Severity::Internal => "internal error",
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Information => "note",
                Severity::Hint => "help",
            };

            delivery::message(format_args!(
                "{severity}[{}]: {}\nerror: Could not render the diagnostic: {error:#}",
                finding.code, finding.message,
            ));

            return Err(error).wrap_err("Could not render the diagnostic.");
        }

        DETAILS.store(true, Ordering::Relaxed);
    }

    Ok(())
}

fn render(diagnostic: &dyn Diagnostic, severity: Severity) -> Result<()> {
    let mut theme = match (delivery::unicode(), console::colors_enabled_stderr()) {
        (true, true) => GraphicalTheme::unicode(),
        (true, false) => GraphicalTheme::unicode_nocolor(),
        (false, true) => GraphicalTheme::ascii(),
        (false, false) => GraphicalTheme::none(),
    };

    if severity == Severity::Internal {
        "internal error:"
    } else {
        "error:"
    }
    .clone_into(&mut theme.characters.error);

    "warning:".clone_into(&mut theme.characters.warning);

    if severity == Severity::Hint {
        "help:"
    } else {
        "note:"
    }
    .clone_into(&mut theme.characters.advice);

    theme.styles.link = miette::ThemeStyles::none().link;

    let mut output = String::new();

    GraphicalReportHandler::new_themed(theme)
        .with_width(usize::from(console::Term::stderr().size().1).max(20))
        .with_links(false)
        .with_wrap_lines(false)
        .without_syntax_highlighting()
        .render_report(&mut output, diagnostic)?;

    delivery::suspend(|| io::stderr().lock().write_all(output.as_bytes())).map_err(Report::from)
}

pub(crate) fn unformatted(path: &Path) {
    delivery::message(format_args!(
        "{}  {}",
        console::style("Needs formatting").for_stderr().yellow(),
        path.display(),
    ));

    DETAILS.store(true, Ordering::Relaxed);
}

pub(crate) fn relative(path: &Path, root: &Path) -> PathBuf {
    let mut prefix = PathBuf::new();

    for ancestor in root.ancestors() {
        if let Ok(path) = path.strip_prefix(ancestor) {
            return prefix.join(path);
        }

        prefix.push("..");
    }

    path.to_owned()
}
