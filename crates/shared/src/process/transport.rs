use std::{
    env::{self, consts::EXE_SUFFIX},
    error::Error,
    fmt::{self, Display},
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use serde::{Serialize, de::DeserializeOwned};

use super::protocol::{self, Handshake, Message, RequestFrame, WorkerError};
use crate::{Completion, Options, error::invalid};

static HOST: OnceLock<PathBuf> = OnceLock::new();
static IDENTITIES: AtomicU64 = AtomicU64::new(1);
const POLL_INTERVAL: Duration = Duration::from_millis(1);

/// A lazy reusable worker client accepting exactly one sequential request at a time.
pub struct Process<Request, Response> {
    identity: u64,
    generation: u64,
    request: u64,
    accepting: bool,
    transport: Option<Transport<Request, Response>>,
}

/// A worker's terminal output or an interruption that discarded its generation.
#[derive(Debug)]
pub enum Outcome<Response> {
    /// A complete operation-specific response, which can contain source errors.
    Response(Response),

    /// Cancellation or deadline expiry terminated and reaped the worker.
    Interrupted(Completion),
}

impl<Request, Response> Process<Request, Response> {
    /// Returns the unique host client identity, independent of an operating system process.
    #[must_use]
    pub const fn identity(&self) -> u64 {
        self.identity
    }

    /// Returns the currently live process generation, or none before startup or after discard.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.transport
            .as_ref()
            .map(|transport| transport.handshake.generation)
    }

    /// Sends a captured request, starting its worker lazily under the original deadline.
    /// Healthy structured invalid-request errors retain the worker; failed transports do not.
    ///
    /// # Errors
    /// Returns startup, invalid request, framing, transport or child cleanup failures.
    pub fn request(
        &mut self,
        worker: &str,
        request: Request,
        options: &Options,
    ) -> io::Result<Outcome<Response>>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        self.request_with(worker, options, move |_, _, _| request)
    }

    /// Constructs a request after startup using the live identities and remaining budget.
    /// The builder runs once immediately before queue submission; the original deadline
    /// continues to cover construction, queueing and serialization.
    ///
    /// # Errors
    /// Returns startup, invalid request, framing, transport or child cleanup failures.
    pub fn request_with(
        &mut self,
        worker: &str,
        options: &Options,
        build: impl FnOnce(u64, u64, Option<Duration>) -> Request,
    ) -> io::Result<Outcome<Response>>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        if !self.accepting {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "worker client has finished",
            ));
        }

        if worker.is_empty() || worker.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker mode must be nonempty and contain no NUL",
            ));
        }

        if self
            .transport
            .as_ref()
            .is_some_and(|transport| transport.handshake.worker != worker)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker client mode does not agree with its live generation",
            ));
        }

        if let Some(completion) = options.interrupted() {
            self.discard()?;

            return Ok(Outcome::Interrupted(completion));
        }

        let response = self.exchange(worker, options, build);

        match response {
            Ok(Exchange::Response(Ok(response))) => Ok(Outcome::Response(response)),

            Ok(Exchange::Response(Err(failure))) => {
                let error = match failure.into_error() {
                    Ok(error) => error,
                    Err(error) => return Err(self.discard_error(error)),
                };

                if protocol::retains_generation(error.kind()) {
                    Err(error)
                } else {
                    Err(self.discard_error(error))
                }
            }

            Ok(Exchange::Interrupted(completion)) => {
                self.discard()?;

                Ok(Outcome::Interrupted(completion))
            }

            Err(error) => {
                if error
                    .get_ref()
                    .is_some_and(|failure| failure.downcast_ref::<CleanupFailure>().is_some())
                {
                    return Err(self.discard_error(error));
                }

                if let Some(completion) = options.interrupted() {
                    if let Some(transport) = &self.transport
                        && let Err(failure) = transport.drain(options, self.request)
                    {
                        return Err(self.discard_error(failure));
                    }

                    self.discard()?;

                    Ok(Outcome::Interrupted(completion))
                } else {
                    Err(self.discard_error(error))
                }
            }
        }
    }

    /// Discards the active generation while permitting later lazy startup.
    ///
    /// # Errors
    /// Returns termination, reaping, transport-thread or logging-thread cleanup failures.
    pub fn discard(&mut self) -> io::Result<()> {
        if let Some(mut transport) = self.transport.take() {
            transport.finish()?;
        }

        Ok(())
    }

    /// Stops acceptance and terminates and reaps this client's worker.
    ///
    /// # Errors
    /// Returns termination, reaping or transport cleanup failures; repeated calls are safe.
    pub fn finish(&mut self) -> io::Result<()> {
        self.accepting = false;
        self.discard()?;

        Ok(())
    }

    fn exchange(
        &mut self,
        worker: &str,
        options: &Options,
        build: impl FnOnce(u64, u64, Option<Duration>) -> Request,
    ) -> io::Result<Exchange<Response>>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        if self.transport.is_none() {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or_else(|| io::Error::other("worker process generation exhausted"))?;

            let handshake = Handshake {
                protocol: protocol::VERSION,
                worker: worker.to_owned(),
                generation: self.generation,
                invocation: self.identity,
            };

            self.transport = Some(Transport::start(handshake, options)?);

            loop {
                if let Some(completion) = options.interrupted() {
                    return Ok(Exchange::Interrupted(completion));
                }

                let transport = self
                    .transport
                    .as_ref()
                    .ok_or_else(|| io::Error::other("worker transport is unavailable"))?;

                match transport.receive(options)? {
                    Some(TransportMessage::Ready) => break,

                    Some(TransportMessage::Response(_)) => {
                        return Err(invalid("worker responded before completing its handshake"));
                    }

                    None => {}
                }
            }
        }

        if let Some(completion) = options.interrupted() {
            return Ok(Exchange::Interrupted(completion));
        }

        self.request = self
            .request
            .checked_add(1)
            .ok_or_else(|| io::Error::other("worker request identity exhausted"))?;

        let frame = RequestFrame {
            request: self.request,
            generation: self.generation,
            invocation: self.identity,
            payload: build(self.request, self.generation, options.remaining()),
        };

        let transport = self
            .transport
            .as_ref()
            .ok_or_else(|| io::Error::other("worker transport is unavailable"))?;

        let mut work = Work {
            frame,
            options: options.clone(),
        };

        loop {
            if let Some(completion) = options.interrupted() {
                return Ok(Exchange::Interrupted(completion));
            }

            match transport
                .requests
                .as_ref()
                .ok_or_else(|| io::Error::other("worker is stopped"))?
                .try_send(work)
            {
                Ok(()) => break,

                Err(TrySendError::Full(pending)) => {
                    work = pending;
                    thread::sleep(pause(options));
                }

                Err(TrySendError::Disconnected(_)) => {
                    return Err(io::Error::other("worker request transport stopped"));
                }
            }
        }

        loop {
            if let Some(completion) = options.interrupted() {
                transport.drain(options, self.request)?;

                return Ok(Exchange::Interrupted(completion));
            }

            match transport.receive(options)? {
                Some(TransportMessage::Response(message)) => match *message {
                    Message::Event(event) => options.report(*event),

                    Message::Result {
                        request,
                        generation,
                        result,
                    } => {
                        if request != self.request || generation != self.generation {
                            return Err(invalid(
                                "worker result does not identify the active request and generation",
                            ));
                        }

                        if let Some(completion) = options.interrupted() {
                            return Ok(Exchange::Interrupted(completion));
                        }

                        return Ok(Exchange::Response(result));
                    }
                },

                Some(TransportMessage::Ready) => {
                    return Err(invalid("worker repeated its startup handshake"));
                }

                None => {}
            }
        }
    }

    fn discard_error(&mut self, error: io::Error) -> io::Error {
        match self.discard() {
            Ok(()) => error,
            Err(cleanup) => append(error, cleanup),
        }
    }
}

impl<Request, Response> Default for Process<Request, Response> {
    fn default() -> Self {
        Self {
            identity: IDENTITIES.fetch_add(1, Ordering::Relaxed),
            generation: 0,
            request: 0,
            accepting: true,
            transport: None,
        }
    }
}

impl<Request, Response> Drop for Process<Request, Response> {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            tracing::error!(client = self.identity, %error, "Could not finish worker client");
        }
    }
}

#[derive(Debug)]
struct CleanupFailure {
    operation: io::Error,
    cleanup: io::Error,
}

impl Display for CleanupFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}; worker cleanup failed: {}",
            self.operation, self.cleanup
        )
    }
}

impl Error for CleanupFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.operation)
    }
}

struct Work<Request> {
    frame: RequestFrame<Request>,
    options: Options,
}

enum Exchange<Response> {
    Response(Result<Response, WorkerError>),
    Interrupted(Completion),
}

enum TransportMessage<Response> {
    Ready,
    Response(Box<Message<Result<Response, WorkerError>>>),
}

struct Transport<Request, Response> {
    handshake: Handshake,
    child: Child,
    requests: Option<SyncSender<Work<Request>>>,
    responses: Option<Receiver<io::Result<TransportMessage<Response>>>>,
    communication: Option<JoinHandle<()>>,
    logging: Option<JoinHandle<io::Result<()>>>,
}

impl<Request, Response> Transport<Request, Response> {
    fn start(handshake: Handshake, options: &Options) -> io::Result<Self>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        options.check()?;
        let path = executable()?;
        options.check()?;

        let child = Command::new(path)
            .args(["worker", &handshake.worker])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let (requests, incoming) = mpsc::sync_channel(1);
        let (outgoing, responses) = mpsc::sync_channel(1);

        let mut transport = Self {
            handshake,
            child,
            requests: Some(requests),
            responses: Some(responses),
            communication: None,
            logging: None,
        };

        if let Err(error) = transport.initialize(incoming, outgoing, options) {
            return Err(match transport.finish() {
                Ok(()) => error,
                Err(cleanup) => append(error, cleanup),
            });
        }

        Ok(transport)
    }

    fn initialize(
        &mut self,
        incoming: Receiver<Work<Request>>,
        outgoing: SyncSender<io::Result<TransportMessage<Response>>>,
        options: &Options,
    ) -> io::Result<()>
    where
        Request: Serialize + Send + 'static,
        Response: DeserializeOwned + Send + 'static,
    {
        options.check()?;

        let input = self
            .child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("worker input is unavailable"))?;

        let output = self
            .child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("worker output is unavailable"))?;

        let errors = self
            .child
            .stderr
            .take()
            .ok_or_else(|| io::Error::other("worker error output is unavailable"))?;

        let worker = self.handshake.worker.clone();
        let generation = self.handshake.generation;

        self.logging = Some(
            thread::Builder::new()
                .name(format!("{worker} logging"))
                .spawn(move || {
                    let mut errors = BufReader::new(errors);
                    let mut buffer = [0; 4096];

                    loop {
                        let count = errors.read(&mut buffer)?;

                        if count == 0 {
                            break;
                        }

                        let message = String::from_utf8_lossy(&buffer[..count]);
                        tracing::warn!(%worker, generation, %message, "Worker stderr");
                    }

                    Ok(())
                })?,
        );

        let handshake = self.handshake.clone();
        let options = options.clone();

        self.communication = Some(
            thread::Builder::new()
                .name(format!("{} transport", handshake.worker))
                .spawn(move || {
                    let mut input = BufWriter::new(input);
                    let mut output = BufReader::new(output);

                    let startup = (|| {
                        write_checked(&mut input, &handshake, &options)?;

                        let received: Handshake =
                            protocol::read_frame(&mut output)?.ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "worker exited before completing its handshake",
                                )
                            })?;

                        if received != handshake {
                            return Err(invalid(
                                "worker startup handshake does not agree with the host",
                            ));
                        }

                        outgoing
                            .send(Ok(TransportMessage::Ready))
                            .map_err(|_| io::Error::other("worker receiver stopped"))?;

                        Ok(())
                    })();

                    if let Err(error) = startup {
                        if outgoing.send(Err(error)).is_err() {
                            return;
                        }

                        return;
                    }

                    while let Ok(work) = incoming.recv() {
                        if let Err(error) = exchange(&work, &mut input, &mut output, &outgoing) {
                            if outgoing.send(Err(error)).is_err() {
                                return;
                            }

                            return;
                        }
                    }
                })?,
        );

        Ok(())
    }

    fn receive(&self, options: &Options) -> io::Result<Option<TransportMessage<Response>>> {
        match self
            .responses
            .as_ref()
            .ok_or_else(|| io::Error::other("worker response transport stopped"))?
            .recv_timeout(pause(options))
        {
            Ok(message) => message.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),

            Err(RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("worker response transport stopped"))
            }
        }
    }

    fn drain(&self, options: &Options, request: u64) -> io::Result<()> {
        if let Some(responses) = &self.responses {
            while let Ok(message) = responses.try_recv() {
                if let TransportMessage::Response(message) = message?
                    && let Message::Event(event) = *message
                {
                    if event.invocation != self.handshake.invocation || event.operation != request {
                        return Err(invalid("worker event does not identify its active request"));
                    }

                    options.report(*event);
                }
            }
        }

        Ok(())
    }

    fn finish(&mut self) -> io::Result<()> {
        self.requests = None;
        self.responses = None;
        let mut failure = None;

        let exited = match self.child.try_wait() {
            Ok(status) => status.is_some(),

            Err(error) => {
                collect(&mut failure, error);

                false
            }
        };

        if !exited && let Err(error) = self.child.kill() {
            match self.child.try_wait() {
                Ok(Some(_)) => {}

                Ok(None) => {
                    return Err(match failure {
                        Some(previous) => append(previous, error),
                        None => error,
                    });
                }

                Err(next) => {
                    let error = append(error, next);

                    return Err(match failure {
                        Some(previous) => append(previous, error),
                        None => error,
                    });
                }
            }
        }

        if let Err(error) = self.child.wait() {
            collect(&mut failure, error);
        }

        if let Some(communication) = self.communication.take()
            && communication.join().is_err()
        {
            collect(&mut failure, io::Error::other("worker transport panicked"));
        }

        if let Some(logging) = self.logging.take() {
            match logging.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => collect(&mut failure, error),
                Err(_) => collect(&mut failure, io::Error::other("worker logging panicked")),
            }
        }

        match failure {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Selects the executable serving shared isolated worker modes.
///
/// # Errors
/// Returns path resolution failures or already-exists when a host was selected previously.
pub fn set_host(path: &Path) -> io::Result<()> {
    HOST.set(path.canonicalize()?).map_err(|_| {
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "worker host is already selected",
        )
    })?;

    Ok(())
}

fn exchange<Request, Response>(
    work: &Work<Request>,
    input: &mut impl Write,
    output: &mut impl BufRead,
    outgoing: &SyncSender<io::Result<TransportMessage<Response>>>,
) -> io::Result<()>
where
    Request: Serialize,
    Response: DeserializeOwned,
{
    write_checked(input, &work.frame, &work.options)?;

    loop {
        let message: Message<Result<Response, WorkerError>> = protocol::read_frame(output)?
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "worker exited without a terminal response",
                )
            })?;

        let finished = match &message {
            Message::Event(event) => {
                if event.invocation != work.frame.invocation
                    || event.operation != work.frame.request
                {
                    return Err(invalid("worker event does not identify its active request"));
                }

                false
            }

            Message::Result {
                request,
                generation,
                ..
            } => {
                if *request != work.frame.request || *generation != work.frame.generation {
                    return Err(invalid(
                        "worker result does not identify its active request and generation",
                    ));
                }

                true
            }
        };

        outgoing
            .send(Ok(TransportMessage::Response(Box::new(message))))
            .map_err(|_| io::Error::other("worker receiver stopped"))?;

        if finished {
            return Ok(());
        }
    }
}

fn write_checked(
    output: &mut impl Write,
    value: &impl Serialize,
    options: &Options,
) -> io::Result<()> {
    struct CheckedWriter<'output, Output> {
        output: &'output mut Output,
        options: &'output Options,
    }

    impl<Output: Write> Write for CheckedWriter<'_, Output> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.options.check()?;

            self.output.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.options.check()?;
            self.output.flush()?;

            Ok(())
        }
    }

    let mut output = CheckedWriter { output, options };
    protocol::write_frame(&mut output, value)?;
    options.check()?;

    Ok(())
}

fn pause(options: &Options) -> Duration {
    options
        .remaining()
        .map_or(POLL_INTERVAL, |remaining| remaining.min(POLL_INTERVAL))
}

fn collect(failure: &mut Option<io::Error>, error: io::Error) {
    *failure = Some(match failure.take() {
        Some(previous) => append(previous, error),
        None => error,
    });
}

fn append(error: io::Error, cleanup: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        CleanupFailure {
            operation: error,
            cleanup,
        },
    )
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
