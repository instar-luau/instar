use std::{
    io::{self, BufRead, BufReader, BufWriter, Stdout, Write},
    marker::PhantomData,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Completion, Event, EventPayload, Reason, error::invalid};

pub(super) const VERSION: u32 = 1;
const FRAME_LIMIT: usize = 64 * 1024 * 1024;

const KINDS: &[io::ErrorKind] = &[
    io::ErrorKind::NotFound,
    io::ErrorKind::PermissionDenied,
    io::ErrorKind::ConnectionRefused,
    io::ErrorKind::ConnectionReset,
    io::ErrorKind::HostUnreachable,
    io::ErrorKind::NetworkUnreachable,
    io::ErrorKind::ConnectionAborted,
    io::ErrorKind::NotConnected,
    io::ErrorKind::AddrInUse,
    io::ErrorKind::AddrNotAvailable,
    io::ErrorKind::NetworkDown,
    io::ErrorKind::BrokenPipe,
    io::ErrorKind::AlreadyExists,
    io::ErrorKind::WouldBlock,
    io::ErrorKind::NotADirectory,
    io::ErrorKind::IsADirectory,
    io::ErrorKind::DirectoryNotEmpty,
    io::ErrorKind::ReadOnlyFilesystem,
    io::ErrorKind::StaleNetworkFileHandle,
    io::ErrorKind::InvalidInput,
    io::ErrorKind::InvalidData,
    io::ErrorKind::TimedOut,
    io::ErrorKind::WriteZero,
    io::ErrorKind::StorageFull,
    io::ErrorKind::NotSeekable,
    io::ErrorKind::QuotaExceeded,
    io::ErrorKind::FileTooLarge,
    io::ErrorKind::ResourceBusy,
    io::ErrorKind::ExecutableFileBusy,
    io::ErrorKind::Deadlock,
    io::ErrorKind::CrossesDevices,
    io::ErrorKind::TooManyLinks,
    io::ErrorKind::InvalidFilename,
    io::ErrorKind::ArgumentListTooLong,
    io::ErrorKind::Interrupted,
    io::ErrorKind::Unsupported,
    io::ErrorKind::UnexpectedEof,
    io::ErrorKind::OutOfMemory,
    io::ErrorKind::Other,
];

/// Startup agreement identifying one worker mode, protocol and process generation.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Handshake {
    /// Wire protocol version.
    pub protocol: u32,

    /// Worker mode served by this executable.
    pub worker: String,

    /// Host-assigned process generation.
    pub generation: u64,

    /// Unique host client identity for event correlation.
    pub invocation: u64,
}

/// One request in a generation; identities increase across a client's restarts.
#[derive(Deserialize, Serialize)]
pub struct RequestFrame<Request> {
    /// Monotonically increasing client request identity.
    pub request: u64,

    /// Process generation that must handle this request.
    pub generation: u64,

    /// Host client identity matching the startup handshake.
    pub invocation: u64,

    /// Operation-specific captured request data.
    pub payload: Request,
}

/// A correlated streamed event or the final result of exactly one request.
#[derive(Deserialize, Serialize)]
pub enum Message<Response> {
    /// A worker event whose invocation and operation identify this client and request.
    Event(Box<Event>),

    /// Terminal result carrying exact request and generation identities.
    Result {
        /// Client request identity.
        request: u64,
        /// Worker process generation.
        generation: u64,
        /// Operation-specific terminal output.
        result: Response,
    },
}

#[derive(Deserialize, Serialize)]
pub(super) struct WorkerError {
    kind: String,
    code: Option<i32>,
    message: String,
    execution: Option<ExecutionError>,
}

#[derive(Deserialize, Serialize)]
enum ExecutionError {
    Reason(Reason),
    Completion(Completion),
}

impl WorkerError {
    pub(super) fn into_error(self) -> io::Result<io::Error> {
        let kind = if let Some(code) = self.code {
            let kind = io::Error::from_raw_os_error(code).kind();

            if format!("{kind:?}") != self.kind {
                return Err(invalid(
                    "worker error category does not match its operating system code",
                ));
            }

            kind
        } else {
            KINDS
                .iter()
                .copied()
                .find(|kind| format!("{kind:?}") == self.kind)
                .ok_or_else(|| invalid("unrecognized worker error category"))?
        };

        if let Some(execution) = self.execution {
            let error = match execution {
                ExecutionError::Reason(reason) => reason.error(),
                ExecutionError::Completion(completion) => completion.error(),
            };

            if error.kind() != kind || error.to_string() != self.message || self.code.is_some() {
                return Err(invalid(
                    "worker execution outcome does not agree with its error metadata",
                ));
            }

            return Ok(error);
        }

        Ok(io::Error::new(kind, self.message))
    }
}

impl From<io::Error> for WorkerError {
    fn from(error: io::Error) -> Self {
        let execution = error.get_ref().and_then(|source| {
            if let Some(reason) = source.downcast_ref::<Reason>() {
                Some(ExecutionError::Reason(*reason))
            } else {
                source
                    .downcast_ref::<Completion>()
                    .map(|completion| ExecutionError::Completion(completion.clone()))
            }
        });

        Self {
            kind: format!("{:?}", error.kind()),
            code: error.raw_os_error(),
            message: error.to_string(),
            execution,
        }
    }
}

/// An owned, clonable emitter correlated to one active worker request.
pub struct Emitter<Response> {
    output: Arc<Mutex<BufWriter<Stdout>>>,
    active: Arc<AtomicBool>,
    invocation: u64,
    request: u64,
    generation: u64,
    response: PhantomData<fn() -> Response>,
}

impl<Response> Emitter<Response> {
    /// Returns the active request and process generation identities, in that order.
    #[must_use]
    pub const fn identities(&self) -> (u64, u64) {
        (self.request, self.generation)
    }

    /// Publishes a payload with this request's event correlation identities.
    ///
    /// # Errors
    /// Returns protocol or output failures, including emission after the terminal result.
    pub fn emit(&self, payload: EventPayload) -> io::Result<()> {
        self.event(Event {
            invocation: self.invocation,
            operation: self.request,
            payload,
        })?;

        Ok(())
    }

    /// Publishes an event whose identities must agree with this active request.
    ///
    /// # Errors
    /// Returns invalid input for mismatched identities or output failures.
    pub fn event(&self, event: Event) -> io::Result<()> {
        if event.invocation != self.invocation || event.operation != self.request {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker event does not identify its active request",
            ));
        }

        let mut output = self
            .output
            .lock()
            .map_err(|_| io::Error::other("worker output lock is poisoned"))?;

        if !self.active.load(Ordering::Acquire) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker request has already completed",
            ));
        }

        let message: Message<()> = Message::Event(Box::new(event));
        write_frame(&mut *output, &message)?;

        Ok(())
    }
}

impl<Response> Clone for Emitter<Response> {
    fn clone(&self) -> Self {
        Self {
            output: Arc::clone(&self.output),
            active: Arc::clone(&self.active),
            invocation: self.invocation,
            request: self.request,
            generation: self.generation,
            response: PhantomData,
        }
    }
}

/// Serves sequential requests using the shared handshake, event and terminal framing.
/// Structured invalid requests retain the session; other handler errors terminate it.
///
/// # Errors
/// Returns startup agreement, framing, input or output failures.
pub fn serve<Request, Response>(
    worker: &str,
    mut handler: impl FnMut(Request, &Emitter<Response>) -> io::Result<Response>,
) -> io::Result<()>
where
    Request: DeserializeOwned,
    Response: Serialize,
{
    let mut input = BufReader::new(io::stdin());
    let output = Arc::new(Mutex::new(BufWriter::new(io::stdout())));

    let handshake: Handshake = read_frame(&mut input)?.ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "worker startup handshake is missing",
        )
    })?;

    if handshake.protocol != VERSION
        || handshake.worker != worker
        || handshake.generation == 0
        || handshake.invocation == 0
    {
        return Err(invalid(
            "worker startup handshake does not agree with the host",
        ));
    }

    {
        let mut output = output
            .lock()
            .map_err(|_| io::Error::other("worker output lock is poisoned"))?;

        write_frame(&mut *output, &handshake)?;
    }

    let mut previous = 0;

    while let Some(frame) = read_frame::<RequestFrame<Request>>(&mut input)? {
        if frame.request <= previous
            || frame.generation != handshake.generation
            || frame.invocation != handshake.invocation
        {
            return Err(invalid(
                "worker request identity does not agree with its active generation",
            ));
        }

        previous = frame.request;

        let emitter = Emitter {
            output: Arc::clone(&output),
            active: Arc::new(AtomicBool::new(true)),
            invocation: frame.invocation,
            request: frame.request,
            generation: frame.generation,
            response: PhantomData,
        };

        let result = handler(frame.payload, &emitter);

        let healthy = result
            .as_ref()
            .err()
            .is_none_or(|error| retains_generation(error.kind()));

        let message = Message::Result {
            request: frame.request,
            generation: frame.generation,
            result: result.map_err(WorkerError::from),
        };

        {
            let mut output = output
                .lock()
                .map_err(|_| io::Error::other("worker output lock is poisoned"))?;

            emitter.active.store(false, Ordering::Release);
            write_frame(&mut *output, &message)?;
        }

        if !healthy {
            return Ok(());
        }
    }

    Ok(())
}

pub(super) fn retains_generation(kind: io::ErrorKind) -> bool {
    matches!(
        kind,
        io::ErrorKind::InvalidInput | io::ErrorKind::InvalidData | io::ErrorKind::Unsupported
    )
}

pub(super) fn read_frame<Value>(input: &mut impl BufRead) -> io::Result<Option<Value>>
where
    Value: DeserializeOwned,
{
    let mut frame = Vec::new();

    loop {
        let buffer = input.fill_buf()?;

        if buffer.is_empty() {
            if frame.is_empty() {
                return Ok(None);
            }

            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "worker exited within a protocol frame",
            ));
        }

        let count = buffer
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(buffer.len(), |offset| offset + 1);

        if count > FRAME_LIMIT.saturating_sub(frame.len()) {
            return Err(invalid("worker protocol frame exceeds the size limit"));
        }

        let finished = buffer[count - 1] == b'\n';
        frame.extend_from_slice(&buffer[..count]);
        input.consume(count);

        if finished {
            return serde_json::from_slice(&frame).map(Some).map_err(invalid);
        }
    }
}

pub(super) fn write_frame(output: &mut impl Write, value: &impl Serialize) -> io::Result<()> {
    struct FrameWriter<'output, Output> {
        output: &'output mut Output,
        length: usize,
    }

    impl<Output: Write> Write for FrameWriter<'_, Output> {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if buffer.len() > FRAME_LIMIT.saturating_sub(self.length) {
                return Err(invalid("worker protocol frame exceeds the size limit"));
            }

            let written = self.output.write(buffer)?;
            self.length += written;

            Ok(written)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.output.flush()?;

            Ok(())
        }
    }

    let mut output = FrameWriter { output, length: 0 };
    serde_json::to_writer(&mut output, value).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.flush()?;

    Ok(())
}
