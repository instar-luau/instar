use std::{
    error::Error,
    fmt::{self, Display},
    io,
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize};

use crate::Event;

/// A prerequisite or coverage reason, separate from cancellation and deadline expiry.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
pub enum Reason {
    /// Syntax required by the request is unsupported.
    UnsupportedSyntax,

    /// A requested language feature is unsupported.
    UnsupportedFeature,

    /// The required declaration or runtime environment is unavailable.
    UnavailableEnvironment,

    /// Requested semantic information has not been established.
    UnavailableSemantics,

    /// Analysis exhausted a complexity or resource limit.
    AnalysisLimit,

    /// A query references expired analysis state.
    StaleQuery,

    /// A prerequisite operation failed.
    FailedPrerequisite,
}

impl Reason {
    /// Creates an error retaining the incomplete-work category.
    #[must_use]
    pub fn error(self) -> io::Error {
        let kind = match self {
            Self::UnsupportedSyntax | Self::UnsupportedFeature => io::ErrorKind::Unsupported,
            Self::UnavailableEnvironment | Self::StaleQuery => io::ErrorKind::InvalidInput,

            Self::UnavailableSemantics | Self::AnalysisLimit | Self::FailedPrerequisite => {
                io::ErrorKind::Other
            }
        };

        io::Error::new(kind, self)
    }
}

impl Display for Reason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "operation incomplete: {self:?}")
    }
}

impl Error for Reason {}

/// Whether an operation established all requested results; source errors are independent.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Completion {
    /// All requested work completed, possibly with source diagnostics.
    Complete,

    /// Available findings are retained alongside every missing coverage reason.
    Incomplete(Vec<Reason>),

    /// The caller cancelled the operation.
    Cancelled,

    /// The invocation's original deadline expired.
    TimedOut,
}

impl Completion {
    /// Combines coverage reasons, with cancellation then timeout taking precedence.
    #[must_use]
    pub fn combine(self, next: Self) -> Self {
        match (self, next) {
            (Self::Cancelled, _) | (_, Self::Cancelled) => Self::Cancelled,
            (Self::TimedOut, _) | (_, Self::TimedOut) => Self::TimedOut,
            (Self::Complete, other) | (other, Self::Complete) => other,

            (Self::Incomplete(mut current), Self::Incomplete(next)) => {
                for reason in next {
                    if !current.contains(&reason) {
                        current.push(reason);
                    }
                }

                Self::Incomplete(current)
            }
        }
    }

    /// Converts an incomplete or interrupted outcome to an operation error.
    /// A complete outcome has no failure category and yields an other-kind error.
    #[must_use]
    pub fn error(&self) -> io::Error {
        let kind = match self {
            Self::Cancelled => io::ErrorKind::Interrupted,
            Self::TimedOut => io::ErrorKind::TimedOut,

            Self::Incomplete(reasons) => reasons
                .first()
                .map_or(io::ErrorKind::Other, |reason| reason.error().kind()),

            Self::Complete => io::ErrorKind::Other,
        };

        io::Error::new(kind, self.clone())
    }
}

impl Display for Completion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Complete => formatter.write_str("complete operation has no failure"),
            Self::Incomplete(reasons) => write!(formatter, "operation incomplete: {reasons:?}"),
            Self::Cancelled => formatter.write_str("operation cancelled"),
            Self::TimedOut => formatter.write_str("operation deadline expired"),
        }
    }
}

impl Error for Completion {}

/// Established information or the concrete reason it is unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Knowledge<Output> {
    /// Established output, including established absence when output is optional.
    Known(Output),

    /// No answer was established for this requested information.
    Unavailable(Reason),
}

/// Coverage of a requested category independent of individual findings.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub enum Availability {
    /// The requested category is fully established.
    Available,

    /// Some information is established but coverage is incomplete.
    Partial(Reason),

    /// Requested information could not be established.
    Unavailable(Reason),
}

/// A cross-thread signal whose children observe parent cancellation independently.
#[derive(Clone, Debug, Default)]
pub struct Cancellation {
    signal: Arc<AtomicBool>,
    parent: Option<Arc<Self>>,
}

impl Cancellation {
    /// Creates an independently cancellable child that also observes this signal.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            signal: Arc::default(),
            parent: Some(Arc::new(self.clone())),
        }
    }

    /// Creates a child signal cancelled when its guard is dropped.
    #[must_use]
    pub fn guard(&self) -> CancellationGuard {
        CancellationGuard {
            cancellation: self.child(),
        }
    }

    /// Requests cancellation for this signal and all its children.
    pub fn cancel(&self) {
        self.signal.store(true, Ordering::Release);
    }

    /// Returns whether this signal or any ancestor requested cancellation.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.signal.load(Ordering::Acquire)
            || self
                .parent
                .as_ref()
                .is_some_and(|parent| parent.requested())
    }
}

/// Cancels its owned child signal on drop without cancelling the parent.
#[derive(Debug)]
pub struct CancellationGuard {
    cancellation: Cancellation,
}

impl CancellationGuard {
    /// Returns the child signal to clone into the guarded operation.
    #[must_use]
    pub const fn cancellation(&self) -> &Cancellation {
        &self.cancellation
    }
}

impl Drop for CancellationGuard {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

/// Shared invocation settings; acceptance establishes the deadline before queueing.
#[derive(Clone)]
pub struct Options {
    /// Cancellation shared through the invocation's child signals.
    pub cancellation: Cancellation,

    /// Absolute invocation deadline, including discovery, startup and serialization.
    pub deadline: Option<Instant>,

    /// Maximum scheduling concurrency for every phase.
    pub jobs: NonZeroUsize,

    /// Synchronous event observer; reporting policy is applied by the caller.
    pub observer: Option<Arc<dyn Fn(Event) + Send + Sync>>,
}

impl Options {
    /// Creates invocation settings with a deadline and fresh cancellation signal.
    #[must_use]
    pub fn new(deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            ..Self::default()
        }
    }

    /// Returns the remaining original invocation budget, or no deadline.
    #[must_use]
    pub fn remaining(&self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    /// Creates an independently cancellable child preserving deadline, jobs and observer.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            cancellation: self.cancellation.child(),
            deadline: self.deadline,
            jobs: self.jobs,
            observer: self.observer.clone(),
        }
    }

    /// Publishes an event without changing operation state.
    pub fn report(&self, event: Event) {
        if let Some(observer) = &self.observer {
            observer(event);
        }
    }

    /// Checks the shared cancellation signal and absolute invocation deadline.
    ///
    /// # Errors
    /// Returns interrupted or timed-out errors when work must stop.
    pub fn check(&self) -> io::Result<()> {
        if let Some(completion) = self.interrupted() {
            return Err(completion.error());
        }

        Ok(())
    }

    /// Returns an interruption outcome without resetting the invocation budget.
    #[must_use]
    pub fn interrupted(&self) -> Option<Completion> {
        if self.cancellation.requested() {
            Some(Completion::Cancelled)
        } else if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            Some(Completion::TimedOut)
        } else {
            None
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            cancellation: Cancellation::default(),
            deadline: None,
            jobs: thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
            observer: None,
        }
    }
}

impl fmt::Debug for Options {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Options")
            .field("cancellation", &self.cancellation)
            .field("deadline", &self.deadline)
            .field("jobs", &self.jobs)
            .field("observer", &self.observer.is_some())
            .finish()
    }
}
