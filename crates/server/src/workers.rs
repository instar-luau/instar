use std::{
    collections::BTreeSet,
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex, Weak, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use instar_analysis::{Cancellation, Options};
use instar_core::{project::Project, snapshot::Snapshot};
use instar_format::Formatter;
use tokio::sync::oneshot;

pub(crate) struct Output<Response> {
    pub(crate) result: io::Result<Response>,
    pub(crate) inputs: BTreeSet<PathBuf>,
}

type Operation = Box<dyn FnOnce(&mut Project, &mut Formatter, &Options, Instant) -> bool + Send>;

struct Job {
    snapshot: Snapshot,
    options: Options,
    started: Instant,
    operation: Operation,
}

pub(crate) struct Guard(Arc<Cancellation>);

impl Drop for Guard {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) struct Workers {
    sender: Mutex<Option<mpsc::SyncSender<Job>>>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    active: Mutex<Vec<Weak<Cancellation>>>,
}

impl Workers {
    pub(crate) fn new(options: &Options) -> io::Result<Self> {
        let jobs = thread::available_parallelism().map_or(1, NonZeroUsize::get);
        let (sender, receiver) = mpsc::sync_channel::<Job>(jobs);
        let receiver = Arc::new(Mutex::new(receiver));

        let workers = Self {
            sender: Mutex::new(Some(sender)),
            handles: Mutex::new(Vec::new()),
            active: Mutex::new(Vec::new()),
        };

        for _ in 0..jobs {
            let receiver = Arc::clone(&receiver);
            let timeout = options.timeout;

            let handle = thread::Builder::new()
                .name("instar-worker".to_owned())
                .spawn(move || {
                    let mut project = Project::new(timeout);
                    let mut formatter = Formatter::default();

                    loop {
                        let Ok(job) = receiver.lock().expect("work queue lock").recv() else {
                            break;
                        };

                        if job.options.cancellation.requested() {
                            continue;
                        }

                        project.apply(&job.snapshot);

                        let _ = (job.operation)(
                            &mut project,
                            &mut formatter,
                            &job.options,
                            job.started,
                        );
                    }
                })?;

            workers
                .handles
                .lock()
                .expect("worker handles lock")
                .push(handle);
        }

        Ok(workers)
    }

    pub(crate) fn submit<Response: Send + 'static>(
        &self,
        snapshot: Snapshot,
        options: Options,
        operation: impl FnOnce(&mut Project, &mut Formatter, &Options, Instant) -> Output<Response>
        + Send
        + 'static,
    ) -> io::Result<(Guard, oneshot::Receiver<Output<Response>>)> {
        let cancellation = Arc::new(options.cancellation.clone());
        let guard = Guard(Arc::clone(&cancellation));
        let mut active = self.active.lock().expect("active work lock");
        active.retain(|signal| signal.strong_count() != 0);
        active.push(Arc::downgrade(&cancellation));
        let (response, receiver) = oneshot::channel();

        let job = Job {
            snapshot,
            options,
            started: Instant::now(),
            operation: Box::new(move |project, formatter, options, started| {
                response
                    .send(operation(project, formatter, options, started))
                    .is_ok()
            }),
        };

        let sender = self.sender.lock().expect("work sender lock");

        sender
            .as_ref()
            .ok_or_else(|| io::Error::other("server is shutting down"))?
            .try_send(job)
            .map_err(|error| io::Error::new(io::ErrorKind::WouldBlock, error.to_string()))?;

        Ok((guard, receiver))
    }

    pub(crate) fn stop(&self) {
        for cancellation in self
            .active
            .lock()
            .expect("active work lock")
            .iter()
            .filter_map(Weak::upgrade)
        {
            cancellation.cancel();
        }

        self.sender.lock().expect("work sender lock").take();
    }

    pub(crate) fn finish(&self) -> io::Result<()> {
        self.stop();
        let mut failed = false;

        for handle in self.handles.lock().expect("worker handles lock").drain(..) {
            failed |= handle.join().is_err();
        }

        if failed {
            Err(io::Error::other("worker thread failed"))
        } else {
            Ok(())
        }
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            tracing::error!(%error, "Worker cleanup failed");
        }
    }
}
