use std::{
    collections::{BTreeMap, VecDeque},
    future::poll_fn,
    io, mem,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{
        Arc, Condvar, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    task::{Poll, Waker},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use instar_shared::{
    Cancellation, Completion, ContextIdentity, Event, EventPayload, FailureKind, OperationFailure,
    Options, Progress, Reason, SourceIdentity, SourceVersion,
};

use super::{
    tasks::{Task, TaskState},
    workers::{Budget, Workers},
};

use crate::{cache::Cache, inputs::ProjectSnapshot, watch::Watch};

const INTERRUPTION_INTERVAL: Duration = Duration::from_millis(10);
const MAPPED_CONTEXT_NAMESPACE: u64 = 1 << 63;

#[derive(Default)]
pub(crate) struct Contexts {
    entries: BTreeMap<(ContextIdentity, SourceVersion, usize), ContextIdentity>,
    next: u64,
}

pub(crate) struct Host {
    pub(crate) snapshot: RwLock<ProjectSnapshot>,
    pub(crate) next_source: AtomicU64,
    pub(crate) next_invocation: AtomicU64,
    pub(crate) worker_epoch: AtomicU64,
    pub(crate) module_contexts: Mutex<Contexts>,
}

impl Host {
    pub(crate) fn source_identity(&self) -> SourceIdentity {
        SourceIdentity::new(self.next_source.fetch_add(1, Ordering::Relaxed))
    }

    pub(crate) fn snapshot(&self) -> ProjectSnapshot {
        self.snapshot
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub(crate) fn module_context(
        &self,
        root: ContextIdentity,
        map: SourceVersion,
        node: usize,
    ) -> io::Result<ContextIdentity> {
        let mut contexts = self
            .module_contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let key = (root, map, node);

        if let Some(context) = contexts.entries.get(&key) {
            return Ok(*context);
        }

        let next = contexts
            .next
            .checked_add(1)
            .filter(|next| *next < MAPPED_CONTEXT_NAMESPACE)
            .ok_or_else(|| io::Error::other("mapped module context identities exhausted"))?;

        let context = ContextIdentity::new(MAPPED_CONTEXT_NAMESPACE | next);
        contexts.entries.insert(key, context);
        contexts.next = next;

        Ok(context)
    }

    pub(crate) fn module_placement(
        &self,
        context: ContextIdentity,
    ) -> Option<(ContextIdentity, SourceVersion, usize)> {
        self.module_contexts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .iter()
            .find_map(|(placement, candidate)| (*candidate == context).then_some(*placement))
    }
}

pub(crate) struct Runtime {
    pub(crate) host: Arc<Host>,
    pub(crate) workers: Workers,
    pub(crate) cache: Cache,
    pub(crate) watch: Arc<Mutex<Watch>>,
    pub(crate) invocation: u64,
    pub(crate) events: Arc<Mutex<Vec<Event>>>,
    pub(crate) observer: Option<Arc<dyn Fn(Event) + Send + Sync>>,
    pub(crate) epoch: u64,
}

impl Runtime {
    fn new(host: Arc<Host>, budget: Arc<Budget>, watch: Arc<Mutex<Watch>>) -> Self {
        Self {
            host,
            workers: Workers::new(budget),
            cache: Cache::default(),
            watch,
            invocation: 0,
            events: Arc::default(),
            observer: None,
            epoch: 0,
        }
    }

    pub(crate) fn identity(&mut self) -> u64 {
        self.host.next_invocation.fetch_add(1, Ordering::Relaxed)
    }

    fn finish(&mut self) -> io::Result<()> {
        self.workers.finish()
    }
}

struct Job {
    options: Options,
    run: Box<dyn FnOnce(&mut Runtime) + Send>,
    reject: Box<dyn FnOnce(io::Error) + Send>,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    capacity: usize,
    stopped: bool,
}

#[derive(Default)]
struct Ready {
    queue: Mutex<Queue>,
    available: Condvar,
}

impl Ready {
    fn next(&self) -> Option<Job> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            if let Some(job) = queue.jobs.pop_front() {
                return Some(job);
            }

            if queue.stopped {
                return None;
            }

            queue = self
                .available
                .wait(queue)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn interrupted(&self) -> Vec<Job> {
        let mut queue = self.queue.lock().unwrap_or_else(PoisonError::into_inner);

        let mut interrupted = Vec::new();
        let mut retained = VecDeque::new();

        for job in queue.jobs.drain(..) {
            if job.options.interrupted().is_some() {
                interrupted.push(job);
            } else {
                retained.push_back(job);
            }
        }

        queue.jobs = retained;

        interrupted
    }
}

struct Active {
    cancellation: Cancellation,
    deadline: Option<Instant>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

#[derive(Default)]
struct Cleanup {
    result: Option<Result<(), (io::ErrorKind, String)>>,
    wakers: Vec<Waker>,
}

pub(crate) struct Execution {
    host: Arc<Host>,
    ready: Arc<Ready>,
    workers: Arc<Mutex<Vec<JoinHandle<io::Result<()>>>>>,
    monitor: Arc<Mutex<Option<JoinHandle<()>>>>,
    active: Arc<Mutex<BTreeMap<u64, Active>>>,
    budget: Arc<Budget>,
    watch: Arc<Mutex<Watch>>,
    stopping: Cancellation,
    cleanup: Arc<Mutex<Cleanup>>,
    cleaning: Mutex<Option<JoinHandle<()>>>,
}

impl Execution {
    pub(crate) fn new(host: Arc<Host>) -> Self {
        Self {
            host,
            ready: Arc::default(),
            workers: Arc::default(),
            monitor: Arc::default(),
            active: Arc::default(),
            budget: Arc::default(),
            watch: Arc::default(),
            stopping: Cancellation::default(),
            cleanup: Arc::default(),
            cleaning: Mutex::new(None),
        }
    }

    pub(crate) fn submit<Output>(
        &self,
        mut options: Options,
        action: impl FnOnce(&mut Runtime, &Options) -> io::Result<Output> + Send + 'static,
    ) -> Task<Output>
    where
        Output: Send + 'static,
    {
        options.cancellation = options.cancellation.child();
        let invocation = self.host.next_invocation.fetch_add(1, Ordering::Relaxed);

        let state = Arc::new(Mutex::new(TaskState {
            result: None,
            waker: None,
        }));

        let events = Arc::new(Mutex::new(Vec::<Event>::new()));

        let task = Task {
            state: Arc::clone(&state),
            options: options.clone(),
            events: Arc::clone(&events),
        };

        let publication = options.observer.clone();
        observe(invocation, &events, &mut options);
        self.admit(self.job(invocation, state, events, publication, options, action));

        task
    }

    pub(crate) fn stop(&self) {
        self.stopping.cancel();

        for task in self
            .active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
        {
            task.cancellation.cancel();
        }

        let jobs = {
            let mut queue = self
                .ready
                .queue
                .lock()
                .unwrap_or_else(PoisonError::into_inner);

            queue.stopped = true;

            queue.jobs.drain(..).collect::<Vec<_>>()
        };

        for job in jobs {
            reject_interrupted(job);
        }

        self.ready.available.notify_all();
    }

    pub(crate) async fn finish(&self) -> io::Result<()> {
        self.stop();
        self.start_cleanup()?;

        poll_fn(|context| {
            let mut cleaning = self.cleaning.lock().unwrap_or_else(PoisonError::into_inner);

            let mut cleanup = self.cleanup.lock().unwrap_or_else(PoisonError::into_inner);

            if cleanup.result.is_some() {
                if cleaning
                    .as_ref()
                    .is_some_and(|thread| !thread.is_finished())
                {
                    context.waker().wake_by_ref();

                    return Poll::Pending;
                }

                if let Some(thread) = cleaning.take()
                    && thread.join().is_err()
                {
                    return Poll::Ready(Err(io::Error::other("project cleanup panicked")));
                }

                return Poll::Ready(match cleanup.result.as_ref() {
                    Some(Ok(())) => Ok(()),
                    Some(Err((kind, message))) => Err(io::Error::new(*kind, message.clone())),
                    None => Err(io::Error::other("project cleanup returned no outcome")),
                });
            }

            if !cleanup
                .wakers
                .iter()
                .any(|waker| waker.will_wake(context.waker()))
            {
                cleanup.wakers.push(context.waker().clone());
            }

            Poll::Pending
        })
        .await
    }

    fn job<Output>(
        &self,
        invocation: u64,
        state: Arc<Mutex<TaskState<Output>>>,
        events: Arc<Mutex<Vec<Event>>>,
        publication: Option<Arc<dyn Fn(Event) + Send + Sync>>,
        options: Options,
        action: impl FnOnce(&mut Runtime, &Options) -> io::Result<Output> + Send + 'static,
    ) -> Job
    where
        Output: Send + 'static,
    {
        let wake_state = Arc::clone(&state);

        let wake = Arc::new(move || {
            let waker = wake_state
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .waker
                .take();

            if let Some(waker) = waker {
                waker.wake();
            }
        });

        self.active
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                invocation,
                Active {
                    cancellation: options.cancellation.clone(),
                    deadline: options.deadline,
                    wake,
                },
            );

        let active = Arc::clone(&self.active);
        let stopping = self.stopping.clone();
        let running = options.clone();
        let recorded = Arc::clone(&events);
        let completed = Arc::clone(&state);

        let run = Box::new(move |runtime: &mut Runtime| {
            if stopping.requested() {
                running.cancellation.cancel();
            }

            runtime.invocation = invocation;
            runtime.events.clone_from(&recorded);
            runtime.observer = publication;

            let result = catch_unwind(AssertUnwindSafe(|| {
                running.check().and_then(|()| {
                    running.report(Event {
                        invocation,
                        operation: invocation,
                        payload: EventPayload::Progress(Progress {
                            phase: "Executing project operation".to_owned(),
                            source: None,
                            completed: 0,
                            total: None,
                        }),
                    });

                    runtime.workers.enter(&running)?;

                    action(runtime, &running)
                })
            }))
            .unwrap_or_else(|_| Err(io::Error::other("project operation panicked")));

            runtime.workers.leave();
            let result = result.map_err(|error| failure(&error, &running, &recorded));
            complete(&completed, result);

            active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&invocation);
        });

        let active = Arc::clone(&self.active);
        let rejected = options.clone();

        let reject = Box::new(move |error: io::Error| {
            complete(&state, Err(failure(&error, &rejected, &events)));

            active
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(&invocation);
        });

        Job {
            options,
            run,
            reject,
        }
    }

    fn admit(&self, job: Job) {
        for interrupted in self.ready.interrupted() {
            reject_interrupted(interrupted);
        }

        let mut queue = self
            .ready
            .queue
            .lock()
            .unwrap_or_else(PoisonError::into_inner);

        let started = if self.stopping.requested() {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "project is stopped",
            ))
        } else if queue.capacity == 0 {
            self.start(job.options.jobs.get()).map(|()| {
                queue.capacity = job.options.jobs.get();
            })
        } else {
            Ok(())
        };

        let admission = started.and_then(|()| {
            job.options.check()?;

            if queue.jobs.len() >= queue.capacity {
                Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "project execution queue is saturated",
                ))
            } else {
                Ok(())
            }
        });

        match admission {
            Ok(()) => {
                queue.jobs.push_back(job);
                self.ready.available.notify_one();
            }

            Err(error) => {
                drop(queue);
                (job.reject)(error);
            }
        }
    }

    fn start(&self, jobs: usize) -> io::Result<()> {
        self.budget.configure(jobs);

        let mut workers = self.workers.lock().unwrap_or_else(PoisonError::into_inner);

        for index in workers.len()..jobs {
            let host = Arc::clone(&self.host);
            let ready = Arc::clone(&self.ready);
            let budget = Arc::clone(&self.budget);
            let watch = Arc::clone(&self.watch);

            workers.push(
                thread::Builder::new()
                    .name(format!("project-{index}"))
                    .spawn(move || {
                        let mut runtime = Runtime::new(host, budget, watch);

                        while let Some(job) = ready.next() {
                            (job.run)(&mut runtime);
                        }

                        runtime.finish()
                    })?,
            );
        }

        let active = Arc::clone(&self.active);
        let stopping = self.stopping.clone();
        let ready = Arc::clone(&self.ready);

        let mut monitor = self.monitor.lock().unwrap_or_else(PoisonError::into_inner);

        if monitor.is_none() {
            *monitor = Some(
                thread::Builder::new()
                    .name("project-cancellation".to_owned())
                    .spawn(move || {
                        loop {
                            for job in ready.interrupted() {
                                reject_interrupted(job);
                            }

                            let (empty, wakes) = {
                                let active = active.lock().unwrap_or_else(PoisonError::into_inner);

                                let wakes = active
                                    .values()
                                    .filter(|task| {
                                        task.cancellation.requested()
                                            || task
                                                .deadline
                                                .is_some_and(|deadline| Instant::now() >= deadline)
                                    })
                                    .map(|task| Arc::clone(&task.wake))
                                    .collect::<Vec<_>>();

                                (active.is_empty(), wakes)
                            };

                            for wake in wakes {
                                wake();
                            }

                            if stopping.requested() && empty {
                                break;
                            }

                            thread::sleep(INTERRUPTION_INTERVAL);
                        }
                    })?,
            );
        }

        Ok(())
    }

    fn start_cleanup(&self) -> io::Result<()> {
        let mut cleaning = self.cleaning.lock().unwrap_or_else(PoisonError::into_inner);

        if cleaning.is_some()
            || self
                .cleanup
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .result
                .is_some()
        {
            return Ok(());
        }

        let workers = Arc::clone(&self.workers);
        let monitor = Arc::clone(&self.monitor);
        let cleanup = Arc::clone(&self.cleanup);
        let watch = Arc::clone(&self.watch);

        *cleaning = Some(
            thread::Builder::new()
                .name("project-cleanup".to_owned())
                .spawn(move || {
                    let workers = join(&workers, &monitor);

                    let watching = watch.lock().unwrap_or_else(PoisonError::into_inner).clear();

                    let result = workers.and(watching);

                    let wakers = {
                        let mut cleanup = cleanup.lock().unwrap_or_else(PoisonError::into_inner);

                        cleanup.result =
                            Some(result.map_err(|error| (error.kind(), error.to_string())));

                        mem::take(&mut cleanup.wakers)
                    };

                    for waker in wakers {
                        waker.wake();
                    }
                })?,
        );

        Ok(())
    }
}

impl Drop for Execution {
    fn drop(&mut self) {
        self.stop();

        if let Some(thread) = self
            .cleaning
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            drop(thread.join());
        } else {
            drop(join(&self.workers, &self.monitor));
        }
    }
}

fn complete<Output>(state: &Mutex<TaskState<Output>>, result: Result<Output, OperationFailure>) {
    let waker = {
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);

        state.result = Some(result);

        state.waker.take()
    };

    if let Some(waker) = waker {
        waker.wake();
    }
}

fn reject_interrupted(job: Job) {
    let error = job
        .options
        .interrupted()
        .unwrap_or(Completion::Cancelled)
        .error();

    (job.reject)(error);
}

fn join(
    workers: &Mutex<Vec<JoinHandle<io::Result<()>>>>,
    monitor: &Mutex<Option<JoinHandle<()>>>,
) -> io::Result<()> {
    let handles = mem::take(&mut *workers.lock().unwrap_or_else(PoisonError::into_inner));

    let mut result = Ok(());

    for worker in handles {
        let finished = worker
            .join()
            .map_err(|_| io::Error::other("project executor panicked"))
            .and_then(|result| result);

        if result.is_ok() {
            result = finished;
        }
    }

    if let Some(monitor) = monitor
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        && monitor.join().is_err()
        && result.is_ok()
    {
        result = Err(io::Error::other("project cancellation monitor panicked"));
    }

    result
}

pub(crate) fn failure(
    error: &io::Error,
    options: &Options,
    events: &Mutex<Vec<Event>>,
) -> OperationFailure {
    let mut failure = error
        .get_ref()
        .and_then(|error| error.downcast_ref::<OperationFailure>())
        .cloned()
        .unwrap_or_else(|| OperationFailure {
            kind: match error.kind() {
                io::ErrorKind::WouldBlock => FailureKind::Busy,

                io::ErrorKind::InvalidInput
                | io::ErrorKind::InvalidData
                | io::ErrorKind::NotFound => FailureKind::InvalidInvocation,

                _ => FailureKind::Operational,
            },
            message: error.to_string(),
            completion: options
                .interrupted()
                .or_else(|| {
                    error
                        .get_ref()
                        .and_then(|error| error.downcast_ref::<Completion>())
                        .cloned()
                })
                .or_else(|| {
                    error
                        .get_ref()
                        .and_then(|error| error.downcast_ref::<Reason>())
                        .map(|reason| Completion::Incomplete(vec![*reason]))
                })
                .or_else(|| match error.kind() {
                    io::ErrorKind::Interrupted => Some(Completion::Cancelled),
                    io::ErrorKind::TimedOut => Some(Completion::TimedOut),
                    _ => None,
                })
                .unwrap_or_else(|| Completion::Incomplete(vec![Reason::FailedPrerequisite])),
            diagnostics: Vec::new(),
            completed_sources: Vec::new(),
            completed_edits: Vec::new(),
        });

    if let Some(interrupted) = options.interrupted() {
        failure.completion = failure.completion.combine(interrupted);
    }

    for event in events.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        match &event.payload {
            EventPayload::Diagnostic(diagnostic) if !failure.diagnostics.contains(diagnostic) => {
                failure.diagnostics.push(diagnostic.clone());
            }

            EventPayload::SourceOutcome(outcome) => {
                failure.retain_source(outcome.clone());
            }

            EventPayload::EditOutcome(outcome) if !failure.completed_edits.contains(outcome) => {
                failure.completed_edits.push(outcome.clone());
            }

            _ => {}
        }
    }

    failure
}

fn observe(invocation: u64, events: &Arc<Mutex<Vec<Event>>>, options: &mut Options) {
    let recorded = Arc::clone(events);
    let publication = options.observer.take();

    options.observer = Some(Arc::new(move |mut event| {
        event.invocation = invocation;
        event.operation = invocation;

        recorded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event.clone());

        if let Some(observer) = &publication {
            observer(event);
        }
    }));
}
