use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    io::{self, Write as _},
    path::Path,
    sync::{Arc, atomic::Ordering},
};

use eyre::{Report, Result, WrapErr as _};
use instar_core::workspace::Error as WorkspaceError;

use miette::{
    Diagnostic, GraphicalReportHandler, GraphicalTheme, LabeledSpan, MietteDiagnostic, NamedSource,
    SourceCode,
};

use super::{
    delivery::{self, HAS_DETAILS, Mode},
    logging::Capture,
};

use crate::{
    batch::{Finding, Severity},
    paths::relative,
};

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

    HAS_DETAILS.store(true, Ordering::Relaxed);
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
            .find(|label| label.is_primary)
            .or_else(|| finding.labels.first())
            .map(|label| label.source.clone());

        for label in finding.labels {
            let span = if label.is_primary {
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

        HAS_DETAILS.store(true, Ordering::Relaxed);
    }

    Ok(())
}

fn render(diagnostic: &dyn Diagnostic, severity: Severity) -> Result<()> {
    let mut theme = match (delivery::is_interactive(), console::colors_enabled_stderr()) {
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
