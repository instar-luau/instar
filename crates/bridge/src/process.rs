use std::{
    env,
    io::{self, BufRead, BufReader, Write},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use instar_analysis::{Options, Reason};

use crate::protocol::{Request, Response};

pub(crate) struct Process {
    child: Child,
    requests: Option<SyncSender<Request>>,
    responses: Receiver<io::Result<Response>>,
    transport: Option<JoinHandle<()>>,
}

pub(crate) enum Outcome {
    Response(Response),
    Interrupted(Reason),
}

impl Process {
    pub(crate) fn start() -> io::Result<Self> {
        let mut child = Command::new(executable()?)
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
                let result = exchange(&request, &mut input, &mut output);
                let failed = result.is_err();

                if outgoing.send(result).is_err() || failed {
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

    pub(crate) fn request(
        &mut self,
        request: Request,
        options: &Options,
        started: Instant,
    ) -> io::Result<Outcome> {
        if let Some(reason) = options.interrupted(started) {
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
                Ok(response) => return response.map(Outcome::Response),
                Err(RecvTimeoutError::Timeout) => {}

                Err(RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("worker transport stopped"));
                }
            }
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        self.requests.take();

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

impl Drop for Process {
    fn drop(&mut self) {
        if let Err(error) = self.stop() {
            eprintln!("could not stop native worker: {error}");
        }
    }
}

fn exchange(
    request: &Request,
    input: &mut impl Write,
    output: &mut impl BufRead,
) -> io::Result<Response> {
    serde_json::to_writer(&mut *input, request)?;
    input.write_all(b"\n")?;
    input.flush()?;
    let mut line = String::new();

    if output.read_line(&mut line)? == 0 {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "native worker exited without a response",
        ));
    }

    let result: Result<Response, String> = serde_json::from_str(&line)?;

    result.map_err(io::Error::other)
}

fn executable() -> io::Result<PathBuf> {
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

    let worker = directory.join(format!("worker{}", env::consts::EXE_SUFFIX));

    if !worker.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("native worker is unavailable: {}", worker.display()),
        ));
    }

    Ok(worker)
}
