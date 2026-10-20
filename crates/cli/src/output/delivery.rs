use std::{
    backtrace::{Backtrace, BacktraceStatus},
    env,
    error::Error,
    fmt,
    io::{self, Write as _},
    panic,
    path::Path,
    sync::OnceLock,
    time::Duration,
};

use clap::{
    ColorChoice,
    builder::styling::{AnsiColor, Styles},
};

use eyre::{EyreHandler, Report, Result, WrapErr as _, bail};
use indicatif::{ProgressBar, ProgressStyle};
use tracing::{Event, Subscriber};
use tracing_error::{ErrorLayer, SpanTrace};

use tracing_subscriber::{
    filter::LevelFilter,
    fmt::{
        self as tracing_format, FmtContext, FormatEvent, FormatFields, FormattedFields,
        format::Writer,
    },
    layer::SubscriberExt as _,
    registry::LookupSpan,
    util::SubscriberInitExt as _,
};

use crate::arguments::Arguments;

static DISPLAY: OnceLock<Display> = OnceLock::new();

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Mode {
    Quiet,
    Normal,
    Verbose,
    Debug,
}

impl Mode {
    pub(super) fn detailed(self) -> bool {
        matches!(self, Self::Verbose | Self::Debug)
    }
}

struct Display {
    mode: Mode,
    progress: ProgressBar,
}

pub(super) struct Capture {
    pub(super) backtrace: Backtrace,
    pub(super) spans: Option<SpanTrace>,
}

impl Capture {
    fn new(debug: bool) -> Self {
        Self {
            backtrace: if debug {
                Backtrace::force_capture()
            } else {
                Backtrace::capture()
            },
            spans: debug.then(SpanTrace::capture),
        }
    }

    pub(super) fn details(&self) -> Vec<String> {
        let mut details = Vec::new();

        if let Some(trace) = &self.spans {
            let trace = trace.to_string();

            if !trace.is_empty() {
                details.push(format!("Span trace:\n{trace}"));
            }
        }

        if self.backtrace.status() == BacktraceStatus::Captured {
            details.push(format!("Backtrace:\n{}", self.backtrace));
        }

        details
    }
}

impl EyreHandler for Capture {
    fn debug(
        &self,
        error: &(dyn Error + 'static),
        formatter: &mut fmt::Formatter<'_>,
    ) -> fmt::Result {
        fmt::Display::fmt(error, formatter)
    }
}

#[derive(Default)]
struct Trace {
    bytes: Vec<u8>,
}

impl io::Write for Trace {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);

        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.bytes.is_empty() {
            suspend(|| io::stderr().lock().write_all(&self.bytes))?;
            self.bytes.clear();
        }

        Ok(())
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        drop(self.flush());
    }
}

struct Log;

impl<SubscriberType, Fields> FormatEvent<SubscriberType, Fields> for Log
where
    SubscriberType: Subscriber + for<'span> LookupSpan<'span>,
    Fields: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, SubscriberType, Fields>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let level = console::style(event.metadata().level()).for_stderr();

        let level = match *event.metadata().level() {
            tracing::Level::ERROR => level.red().bold(),
            tracing::Level::WARN => level.yellow().bold(),
            tracing::Level::DEBUG => level.cyan(),
            tracing::Level::TRACE => level.dim(),
            tracing::Level::INFO => level,
        };

        write!(writer, "{level}  ")?;

        context
            .field_format()
            .format_fields(writer.by_ref(), event)?;

        if let Some(scope) = context.event_scope() {
            for span in scope.from_root() {
                write!(writer, " {}", span.name())?;

                let extensions = span.extensions();

                if let Some(fields) = extensions.get::<FormattedFields<Fields>>()
                    && !fields.is_empty()
                {
                    write!(writer, "{{{fields}}}")?;
                }
            }
        }

        writeln!(writer)
    }
}

pub(crate) fn initialize(arguments: &Arguments) -> Result<()> {
    match arguments.color.unwrap_or_default() {
        ColorChoice::Always => console::set_colors_enabled_stderr(true),
        ColorChoice::Never => console::set_colors_enabled_stderr(false),
        ColorChoice::Auto => {}
    }

    let mode = if arguments.quiet {
        Mode::Quiet
    } else if arguments.debug {
        Mode::Debug
    } else if arguments.verbose {
        Mode::Verbose
    } else {
        Mode::Normal
    };

    if DISPLAY
        .set(Display {
            mode,
            progress: if mode == Mode::Quiet || !interactive() {
                ProgressBar::hidden()
            } else {
                ProgressBar::new_spinner()
            },
        })
        .is_err()
    {
        bail!("Internal error: output is already initialized.");
    }

    eyre::set_hook(Box::new(move |_| {
        Box::new(if mode == Mode::Debug {
            Capture::new(true)
        } else {
            Capture {
                backtrace: Backtrace::disabled(),
                spans: None,
            }
        })
    }))
    .map_err(Report::msg)
    .wrap_err("Could not initialize error reporting.")?;

    panic::set_hook(Box::new(move |information| {
        let payload = information
            .payload()
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| information.payload().downcast_ref::<&str>().copied())
            .unwrap_or("Unknown panic payload.");

        let message = information.location().map_or_else(
            || format!("Internal error: {payload}"),
            |location| format!("Internal error: {payload}\n{location}"),
        );

        super::report::panic(&message, &Capture::new(mode == Mode::Debug));
    }));

    tracing_subscriber::registry()
        .with(ErrorLayer::default())
        .with(
            tracing_format::layer()
                .event_format(Log)
                .with_writer(Trace::default)
                .with_ansi(console::colors_enabled_stderr()),
        )
        .with(LevelFilter::from_level(if mode == Mode::Debug {
            tracing::Level::DEBUG
        } else {
            tracing::Level::WARN
        }))
        .try_init()
        .map_err(Report::msg)
        .wrap_err("Could not initialize logging.")?;

    Ok(())
}

pub(super) fn mode() -> Mode {
    display().mode
}

pub(super) fn progress() -> ProgressBar {
    display().progress.clone()
}

pub(super) fn suspend<Value>(execute: impl FnOnce() -> Value) -> Value {
    display().progress.suspend(execute)
}

pub(super) fn message(message: impl fmt::Display) {
    suspend(|| eprintln!("{message}"));
}

pub(super) fn interactive() -> bool {
    console::Term::stderr().is_term()
        && env::var_os("TERM").is_none_or(|terminal| terminal != "dumb")
}

pub(super) fn unicode() -> bool {
    interactive()
}

pub(super) fn ellipsis() -> &'static str {
    if unicode() { "…" } else { "..." }
}

pub(super) fn separator() -> &'static str {
    if unicode() { " · " } else { "; " }
}

pub(crate) fn root(path: &Path) {
    if mode().detailed() {
        file("Project", path, 9);
    }
}

pub(super) fn file(action: &str, path: &Path, width: usize) {
    if mode().detailed() {
        message(format_args!(
            "{}{:padding$}{}",
            console::style(action).for_stderr().cyan().bold(),
            "",
            path.display(),
            padding = width.saturating_sub(console::measure_text_width(action)),
        ));
    }
}

pub(crate) fn workers(count: usize) {
    if mode().detailed() {
        message(format_args!(
            "{}  {count}\n",
            console::style("Workers").for_stderr().cyan().bold(),
        ));
    }
}

pub(crate) fn watching() {
    if mode() != Mode::Quiet {
        message(format_args!(
            "\n{}{}",
            console::style("Watching for changes")
                .for_stderr()
                .cyan()
                .bold(),
            ellipsis(),
        ));
    }
}

pub(crate) fn generator(source: &str, line: &str) {
    let name = source.rsplit(['/', '\\']).next().unwrap_or(source);
    let name = neutralize(name);
    let line = neutralize(line);
    tracing::debug!(generator = source, "Received generator output");
    message(format_args!("[{name}] {line}"));
}

fn neutralize(text: &str) -> String {
    let text = console::strip_ansi_codes(text);

    let mut output = String::new();

    for character in text.chars() {
        if character.is_control() && character != '\t' {
            output.extend(character.escape_default());
        } else {
            output.push(character);
        }
    }

    output
}

pub(crate) fn generation(phase: &str, target: Option<&str>) -> Result<()> {
    let bar = progress();

    bar.set_style(
        ProgressStyle::with_template("{msg}")
            .wrap_err("Could not configure the progress display.")?,
    );

    let message = target.map_or_else(
        || format!("{phase}{}", ellipsis()),
        |target| {
            let width = usize::from(console::Term::stderr().size().1);
            let available = width.saturating_sub(console::measure_text_width(phase) + 2);

            if available <= ellipsis().len() {
                phase.to_owned()
            } else {
                format!(
                    "{phase}  {}",
                    console::truncate_str(target, available, ellipsis())
                )
            }
        },
    );

    bar.set_message(
        console::style(message)
            .for_stderr()
            .cyan()
            .bold()
            .to_string(),
    );

    if !bar.is_hidden() {
        bar.enable_steady_tick(Duration::from_millis(100));
    }

    Ok(())
}

pub(super) fn plural(count: usize, singular: &'static str, plural: &'static str) -> &'static str {
    if count == 1 { singular } else { plural }
}

pub(crate) fn styles() -> Styles {
    Styles::styled()
        .header(AnsiColor::Cyan.on_default().bold())
        .usage(AnsiColor::Cyan.on_default().bold())
        .literal(AnsiColor::Green.on_default().bold())
        .error(AnsiColor::Red.on_default().bold())
        .valid(AnsiColor::Green.on_default())
        .invalid(AnsiColor::Yellow.on_default())
}

fn display() -> &'static Display {
    DISPLAY
        .get()
        .expect("Internal error: output is not initialized.")
}
