use std::{
    backtrace::{Backtrace, BacktraceStatus},
    error::Error,
    fmt,
    io::{self, Write as _},
    panic,
};

use eyre::{EyreHandler, Report, Result, WrapErr as _};
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

use super::{
    delivery::{self, Mode},
    diagnostics,
};

pub(super) struct Capture {
    backtrace: Backtrace,
    spans: Option<SpanTrace>,
}

impl Capture {
    fn new(is_debug: bool) -> Self {
        Self {
            backtrace: if is_debug {
                Backtrace::force_capture()
            } else {
                Backtrace::capture()
            },
            spans: is_debug.then(SpanTrace::capture),
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
            delivery::suspend(|| io::stderr().lock().write_all(&self.bytes))?;
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

pub(super) fn initialize(mode: Mode) -> Result<()> {
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

        diagnostics::panic(&message, &Capture::new(mode == Mode::Debug));
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
