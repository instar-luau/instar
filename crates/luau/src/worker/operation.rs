use std::{
    io,
    sync::Arc,
    time::{Duration, Instant},
};

use instar_shared::{Cancellation, EventPayload, LinePosition, LogLevel, OperationalLog, Progress};

use crate::{
    boundary,
    conversion::{self, Captured},
};

type Events = Arc<dyn Fn(EventPayload) -> io::Result<()> + Send + Sync>;

pub(crate) struct NativeOperation {
    pub captured: Arc<Captured>,
    deadline: Option<Instant>,
    cancellation: Cancellation,
    events: Events,
}

impl NativeOperation {
    pub(crate) fn new(
        captured: Arc<Captured>,
        remaining_budget: Option<Duration>,
        started: Instant,
        events: impl Fn(EventPayload) -> io::Result<()> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        let deadline = remaining_budget
            .map(|remaining| {
                started.checked_add(remaining).ok_or_else(|| {
                    conversion::invalid("worker deadline exceeds supported clock range")
                })
            })
            .transpose()?;

        Ok(Self {
            captured,
            deadline,
            cancellation: Cancellation::default(),
            events: Arc::new(events),
        })
    }

    pub(crate) fn progress(
        &self,
        phase: &str,
        source: &str,
        completed: usize,
        total: usize,
    ) -> io::Result<()> {
        if completed > total {
            return Err(conversion::invalid(
                "native progress exceeds its established total",
            ));
        }

        let source = if source.is_empty() {
            if self.captured.analysis.is_none() {
                self.captured
                    .documents
                    .values()
                    .next()
                    .map(|source| source.version)
            } else {
                None
            }
        } else {
            Some(self.captured.document(source)?.version)
        };

        (self.events)(EventPayload::Progress(Progress {
            phase: phase.to_owned(),
            source,
            completed,
            total: Some(total),
        }))
    }

    pub(crate) fn read_module(&self, name: &str) -> boundary::NativeModule {
        self.captured.modules.get(name).map_or_else(
            || boundary::NativeModule {
                found: false,
                revision: 0,
                sites: Vec::new(),
            },
            |module| boundary::NativeModule {
                found: true,
                revision: module.source.source.version.revision,
                sites: module.imports.iter().map(conversion::import_site).collect(),
            },
        )
    }

    pub(crate) fn read_source(&self, name: &str) -> boundary::NativeSource {
        boundary::NativeSource {
            classes: self
                .captured
                .modules
                .get(name)
                .and_then(|module| module.environment.as_ref())
                .map_or_else(Vec::new, conversion::native_classes),
        }
    }

    pub(crate) fn read_document(&self, name: &str) -> boundary::NativeDocument {
        self.captured.documents.get(name).map_or_else(
            || boundary::NativeDocument {
                found: false,
                text: String::new(),
                revision: 0,
            },
            |source| boundary::NativeDocument {
                found: true,
                text: source.text.to_string(),
                revision: source.version.revision,
            },
        )
    }

    pub(crate) fn revision(&self, name: &str) -> io::Result<u64> {
        Ok(self.captured.document(name)?.version.revision)
    }

    pub(crate) fn offset(&self, name: &str, line: usize, column: usize) -> io::Result<usize> {
        self.captured.offset(name, LinePosition { line, column })
    }

    pub(crate) fn require(&self, name: &str, start: usize, end: usize) -> bool {
        self.captured.modules.get(name).is_some_and(|module| {
            module.imports.iter().any(|import| {
                import.site.call.span.start == start && import.site.call.span.end == end
            })
        })
    }

    pub(crate) fn requested(&self) -> bool {
        self.cancellation.requested()
    }

    pub(crate) fn remaining_nanoseconds(&self) -> u64 {
        self.deadline.map_or(u64::MAX, |deadline| {
            u64::try_from(
                deadline
                    .saturating_duration_since(Instant::now())
                    .as_nanos(),
            )
            .unwrap_or(u64::MAX - 1)
        })
    }

    pub(crate) fn finding(&self, diagnostic: &boundary::NativeDiagnostic) -> io::Result<()> {
        (self.events)(EventPayload::Diagnostic(conversion::diagnostic(
            diagnostic,
            &self.captured,
        )?))
    }

    pub(crate) fn log(&self, level: u8, message: &str) -> io::Result<()> {
        let level = match level {
            0 => LogLevel::Trace,
            1 => LogLevel::Debug,
            2 => LogLevel::Information,
            3 => LogLevel::Warning,
            4 => LogLevel::Error,
            _ => return Err(conversion::invalid("unknown native log level")),
        };

        (self.events)(EventPayload::Log(OperationalLog {
            level,
            message: message.to_owned(),
        }))
    }
}
