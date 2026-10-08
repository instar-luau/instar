use std::{
    fmt, io,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::Progress;

/// Why analysis could not produce a complete answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Reason {
    /// The caller requested cancellation.
    Cancelled,

    /// The analysis budget expired.
    Timeout,

    /// Required environment support is unavailable.
    Unsupported,

    /// Declarations could not establish a valid environment.
    Environment,

    /// Native analysis exceeded a complexity limit or failed internally.
    Analysis,
}

impl Reason {
    /// Converts an incomplete operation into an error for fallible entry points.
    #[must_use]
    pub fn error(self) -> io::Error {
        let kind = match self {
            Self::Cancelled => io::ErrorKind::Interrupted,
            Self::Timeout => io::ErrorKind::TimedOut,
            Self::Unsupported => io::ErrorKind::Unsupported,
            Self::Environment => io::ErrorKind::InvalidData,
            Self::Analysis => io::ErrorKind::Other,
        };

        io::Error::new(kind, format!("operation incomplete: {self:?}"))
    }
}

/// Whether the result covers all selected entries and their dependencies.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Completion {
    /// Analysis finished; diagnostics may still contain errors.
    Complete,

    /// Partial diagnostics must not be interpreted as success.
    Incomplete(Reason),
}

impl Completion {
    /// Keeps the first incomplete phase, with cancellation and timeout taking precedence.
    #[must_use]
    pub fn combine(self, next: Self) -> Self {
        match (self, next) {
            (Self::Incomplete(Reason::Cancelled), _) | (_, Self::Incomplete(Reason::Cancelled)) => {
                Self::Incomplete(Reason::Cancelled)
            }

            (Self::Incomplete(Reason::Timeout), _) | (_, Self::Incomplete(Reason::Timeout)) => {
                Self::Incomplete(Reason::Timeout)
            }

            (Self::Complete, other) => other,
            (current, _) => current,
        }
    }
}

/// A clonable cancellation signal that can be raised from another thread.
#[derive(Clone, Debug, Default)]
pub struct Cancellation(Arc<AtomicBool>);

impl Cancellation {
    /// Requests cancellation of analyses using this signal.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    /// Whether cancellation was requested.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Caller-supplied limits for one project analysis.
#[derive(Clone)]
pub struct Options {
    /// Total elapsed budget, including host preparation and declarations.
    ///
    /// Native analysis runs in an isolated worker that is terminated on interruption.
    /// Host preparation checks cancellation between phases and during traversal;
    /// individual parser and native configuration calls are not forcibly interrupted.
    pub timeout: Duration,

    /// Maximum concurrent formatting or syntax-lint jobs; absent uses the service default.
    pub jobs: Option<NonZeroUsize>,

    /// Shared cancellation signal.
    pub cancellation: Cancellation,

    /// Optional synchronous progress observer, preserved across remaining budgets.
    pub progress: Option<Arc<dyn Fn(Progress) + Send + Sync>>,
}

impl fmt::Debug for Options {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Options")
            .field("timeout", &self.timeout)
            .field("jobs", &self.jobs)
            .field("cancellation", &self.cancellation)
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

impl Options {
    /// Creates limits with a fresh cancellation signal.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            jobs: None,
            cancellation: Cancellation::default(),
            progress: None,
        }
    }

    /// Reports an observed phase boundary without changing analysis state.
    pub fn report(
        &self,
        phase: &str,
        completed: usize,
        total: Option<usize>,
        source: Option<&str>,
    ) {
        if let Some(observer) = &self.progress {
            observer(Progress {
                phase: phase.to_owned(),
                completed,
                total,
                source: source.map(str::to_owned),
            });
        }
    }

    /// Checks cancellation and the elapsed operation budget.
    ///
    /// # Errors
    /// Returns interrupted or timed-out errors when the operation must stop.
    pub fn check(&self, started: Instant) -> io::Result<()> {
        match self.interrupted(started) {
            Some(reason) => Err(reason.error()),
            None => Ok(()),
        }
    }

    /// Returns the interruption reason at the current phase boundary.
    #[must_use]
    pub fn interrupted(&self, started: Instant) -> Option<Reason> {
        if self.cancellation.requested() {
            Some(Reason::Cancelled)
        } else if started.elapsed() >= self.timeout {
            Some(Reason::Timeout)
        } else {
            None
        }
    }

    /// Returns limits carrying the unused portion of the current budget.
    #[must_use]
    pub fn remaining(&self, started: Instant) -> Self {
        Self {
            timeout: self.timeout.saturating_sub(started.elapsed()),
            jobs: self.jobs,
            cancellation: self.cancellation.clone(),
            progress: self.progress.clone(),
        }
    }
}
