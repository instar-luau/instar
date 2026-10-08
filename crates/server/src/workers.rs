use std::{
    collections::BTreeSet,
    io,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex, Weak, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use instar_analysis::{Cancellation, Completion, Options};
use instar_core::{project::Project, snapshot::Snapshot};
use instar_format::Formatter;
use tokio::sync::oneshot;

pub(crate) struct Output {
    pub(crate) text: io::Result<Option<(Arc<str>, String)>>,
    pub(crate) inputs: BTreeSet<PathBuf>,
}

struct Job {
    snapshot: Snapshot,
    path: PathBuf,
    options: Options,
    started: Instant,
    response: oneshot::Sender<Output>,
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
                .name("instar-formatting".to_owned())
                .spawn(move || {
                    let mut project = Project::new(timeout);
                    let mut formatter = Formatter::default();

                    loop {
                        let Ok(job) = receiver.lock().expect("format queue lock").recv() else {
                            break;
                        };

                        if job.options.cancellation.requested() {
                            continue;
                        }

                        project.apply(&job.snapshot);
                        let text = format(&job, &mut project, &mut formatter);

                        let inputs = if text.is_ok() {
                            project
                                .configuration(job.path.parent().expect("absolute source parent"))
                                .map_or_else(
                                    |_| project.watch_inputs(),
                                    |settings| {
                                        let mut inputs = settings.inputs.clone();
                                        inputs.insert(job.path.clone());

                                        inputs
                                    },
                                )
                        } else {
                            project.watch_inputs()
                        };

                        let output = Output { text, inputs };

                        drop(job.response.send(output));
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

    pub(crate) fn submit(
        &self,
        snapshot: Snapshot,
        path: PathBuf,
        options: Options,
    ) -> io::Result<(Guard, oneshot::Receiver<Output>)> {
        let cancellation = Arc::new(options.cancellation.clone());
        let guard = Guard(Arc::clone(&cancellation));
        let mut active = self.active.lock().expect("active work lock");
        active.retain(|signal| signal.strong_count() != 0);
        active.push(Arc::downgrade(&cancellation));
        let (response, receiver) = oneshot::channel();

        let job = Job {
            snapshot,
            path,
            options,
            started: Instant::now(),
            response,
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
            Err(io::Error::other("formatting worker thread failed"))
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

fn format(
    job: &Job,
    project: &mut Project,
    formatter: &mut Formatter,
) -> io::Result<Option<(Arc<str>, String)>> {
    job.options.check(job.started)?;

    let Some(configuration) =
        project.formatting_configuration(&job.path, &job.options.remaining(job.started))?
    else {
        return Ok(None);
    };

    let source = project.source(&job.path)?;

    let result = formatter.format(
        source.text.as_bytes(),
        &configuration,
        &job.options.remaining(job.started),
    )?;

    job.options.check(job.started)?;

    if let Completion::Incomplete(reason) = result.completion {
        return Err(reason.error());
    }

    if !result.diagnostics.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            result
                .diagnostics
                .iter()
                .map(|diagnostic| diagnostic.message.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }

    let text = result
        .output
        .ok_or_else(|| io::Error::other("formatter returned no output"))?;

    let text = String::from_utf8(text)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;

    Ok(Some((source.text, text)))
}
