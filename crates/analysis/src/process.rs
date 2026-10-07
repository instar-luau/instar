//! Cancellable typed requests to isolated analysis workers.

use std::{
    env::{self, consts::EXE_SUFFIX},
    io::{self, BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        OnceLock,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Options, Progress, Reason, error::Failure};

static HOST: OnceLock<PathBuf> = OnceLock::new();

/// An isolated worker with deadline-aware request transport.
pub struct Process<Request, Response> {
    child: Child,
    requests: Option<SyncSender<Request>>,
    responses: Receiver<io::Result<Message<Response>>>,
    transport: Option<JoinHandle<()>>,
}

/// A worker response or an interrupted operation.
pub enum Outcome<Response> {
    /// A complete worker response.
    Response(Response),

    /// The worker was stopped before completing the request.
    Interrupted(Reason),
}

/// A streamed worker update or its terminal response.
#[derive(Deserialize, Serialize)]
pub enum Message<Response> {
    /// An observed work boundary before the terminal response.
    Progress(Progress),

    /// The final result of one request.
    Response(Result<Response, Failure>),
}

impl<Request, Response> Process<Request, Response> {
    /// Starts an isolated worker mode in the host executable.
    ///
    /// # Errors
    /// Returns executable discovery, process creation, or transport failures.
    pub fn start(name: &str) -> io::Result<Self>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        let path = executable()?;
        tracing::debug!(worker = name, path = %path.display(), "Starting worker");

        let mut child = Command::new(path)
            .args(["worker", name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        tracing::debug!(worker = name, process = child.id(), "Worker started");

        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker input is unavailable"))?;

        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker output is unavailable"))?;

        let (requests, incoming) = mpsc::sync_channel::<Request>(1);
        let (outgoing, responses) = mpsc::sync_channel(1);

        let transport = thread::spawn(move || {
            let mut input = BufWriter::new(input);
            let mut output = BufReader::new(output);

            while let Ok(request) = incoming.recv() {
                if let Err(error) = exchange(request, &mut input, &mut output, &outgoing) {
                    if outgoing.send(Err(error)).is_err() {
                        return;
                    }

                    break;
                }
            }
        });

        Ok(Self {
            child,
            requests: Some(requests),
            responses,
            transport: Some(transport),
        })
    }

    /// Sends a request and stops the worker when cancelled or timed out.
    ///
    /// # Errors
    /// Returns transport, worker, or process cleanup failures.
    pub fn request(
        &mut self,
        request: Request,
        options: &Options,
        started: Instant,
    ) -> io::Result<Outcome<Response>> {
        if let Some(reason) = options.interrupted(started) {
            tracing::debug!(process = self.child.id(), ?reason, "Request interrupted");
            self.stop()?;

            return Ok(Outcome::Interrupted(reason));
        }

        tracing::debug!(process = self.child.id(), "Sending worker request");

        self.requests
            .as_ref()
            .ok_or_else(|| io::Error::other("worker is stopped"))?
            .send(request)
            .map_err(|_| io::Error::other("worker transport is unavailable"))?;

        loop {
            if let Some(reason) = options.interrupted(started) {
                tracing::debug!(process = self.child.id(), ?reason, "Request interrupted");
                self.stop()?;

                return Ok(Outcome::Interrupted(reason));
            }

            let remaining = options.timeout.saturating_sub(started.elapsed());

            match self
                .responses
                .recv_timeout(remaining.min(Duration::from_millis(1)))
            {
                Ok(response) => {
                    if let Some(reason) = options.interrupted(started) {
                        tracing::debug!(process = self.child.id(), ?reason, "Request interrupted");
                        self.stop()?;

                        return Ok(Outcome::Interrupted(reason));
                    }

                    match response? {
                        Message::Progress(progress) => {
                            if let Some(observer) = &options.progress {
                                observer(progress);
                            }
                        }

                        Message::Response(Ok(response)) => {
                            tracing::debug!(
                                process = self.child.id(),
                                elapsed_seconds = started.elapsed().as_secs_f64(),
                                "Worker request completed"
                            );

                            return Ok(Outcome::Response(response));
                        }

                        Message::Response(Err(error)) => {
                            tracing::debug!(process = self.child.id(), "Worker request failed");

                            return Err(io::Error::try_from(error)?);
                        }
                    }
                }

                Err(RecvTimeoutError::Timeout) => {}

                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("worker transport stopped"));
                }
            }
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        self.requests.take();
        self.responses = mpsc::sync_channel(1).1;

        let killed = if self.child.try_wait()?.is_some() {
            Ok(())
        } else {
            tracing::debug!(process = self.child.id(), "Stopping worker");

            self.child.kill()
        };

        self.child.wait()?;

        if let Some(transport) = self.transport.take() {
            transport
                .join()
                .map_err(|_| io::Error::other("worker transport panicked"))?;

            tracing::debug!(process = self.child.id(), "Worker transport joined");
        }

        killed
    }
}

impl<Request, Response> Drop for Process<Request, Response> {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("could not stop analysis worker: {error}");
        }
    }
}

/// Selects the executable that serves isolated worker requests.
///
/// # Errors
/// Returns path resolution failures or an error if the host is already selected.
pub fn set_host(path: &Path) -> io::Result<()> {
    HOST.set(path.canonicalize()?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "worker host is already selected",
        )
    })
}

fn exchange<Request: Serialize, Response: DeserializeOwned>(
    request: Request,
    input: &mut impl Write,
    output: &mut impl BufRead,
    outgoing: &SyncSender<io::Result<Message<Response>>>,
) -> io::Result<()> {
    serde_json::to_writer(&mut *input, &request)?;
    input.write_all(b"\n")?;
    input.flush()?;
    drop(request);

    loop {
        let mut line = String::new();

        if output.read_line(&mut line)? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "analysis worker exited without a response",
            ));
        }

        let message: Message<Response> = serde_json::from_str(&line)?;
        let finished = matches!(message, Message::Response(_));

        outgoing
            .send(Ok(message))
            .map_err(|_| io::Error::other("worker receiver stopped"))?;

        if finished {
            return Ok(());
        }
    }
}

fn executable() -> io::Result<PathBuf> {
    if let Some(path) = HOST.get() {
        return Ok(path.clone());
    }

    let executable = env::current_exe()?;

    let directory = executable
        .parent()
        .ok_or_else(|| io::Error::other("executable directory is unavailable"))?;

    let directory = if directory
        .file_name()
        .is_some_and(|name| name == "deps" || name == "examples")
    {
        directory.parent()
    } else if directory.file_name().is_some_and(|name| name == "out")
        && directory
            .ancestors()
            .nth(3)
            .is_some_and(|path| path.file_name().is_some_and(|name| name == "build"))
    {
        directory.ancestors().nth(4)
    } else {
        Some(directory)
    }
    .ok_or_else(|| io::Error::other("worker executable directory is unavailable"))?;

    let worker = directory.join(format!("instar{EXE_SUFFIX}"));

    if !worker.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("worker host is unavailable: {}", worker.display()),
        ));
    }

    Ok(worker)
}
