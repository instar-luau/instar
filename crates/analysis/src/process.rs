//! Cancellable typed requests to isolated analysis workers.

use std::{
    env,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Options, Reason};

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
    Progress(crate::Progress),

    /// The final result of one request.
    Response(Result<Response, crate::error::Failure>),
}

impl<Request, Response> Process<Request, Response> {
    /// Starts a sibling worker executable.
    ///
    /// # Errors
    /// Returns executable discovery, process creation, or transport failures.
    pub fn start(name: &str) -> io::Result<Self>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        let mut child = Command::new(executable(name)?)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;

        let mut input = child
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
            let mut output = BufReader::new(output);

            while let Ok(request) = incoming.recv() {
                if let Err(error) = exchange(&request, &mut input, &mut output, &outgoing) {
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
            self.stop()?;

            return Ok(Outcome::Interrupted(reason));
        }

        self.requests
            .as_ref()
            .ok_or_else(|| io::Error::other("worker is stopped"))?
            .send(request)
            .map_err(|_| io::Error::other("worker transport is unavailable"))?;

        loop {
            if let Some(reason) = options.interrupted(started) {
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
                        self.stop()?;

                        return Ok(Outcome::Interrupted(reason));
                    }

                    match response? {
                        Message::Progress(progress) => {
                            if let Some(observer) = &options.progress {
                                observer(progress);
                            }
                        }

                        Message::Response(Ok(response)) => return Ok(Outcome::Response(response)),
                        Message::Response(Err(error)) => return Err(io::Error::try_from(error)?),
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

        let killed = match self.child.try_wait()? {
            Some(_) => Ok(()),
            None => self.child.kill(),
        };

        self.child.wait()?;

        if let Some(transport) = self.transport.take() {
            transport
                .join()
                .map_err(|_| io::Error::other("worker transport panicked"))?;
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

fn exchange<Request: Serialize, Response: DeserializeOwned>(
    request: &Request,
    input: &mut impl Write,
    output: &mut impl BufRead,
    outgoing: &SyncSender<io::Result<Message<Response>>>,
) -> io::Result<()> {
    serde_json::to_writer(&mut *input, request)?;
    input.write_all(b"\n")?;
    input.flush()?;

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

fn executable(name: &str) -> io::Result<PathBuf> {
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

    let worker = directory.join(format!("{name}{}", env::consts::EXE_SUFFIX));

    if !worker.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("analysis worker is unavailable: {}", worker.display()),
        ));
    }

    Ok(worker)
}
