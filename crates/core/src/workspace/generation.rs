use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs,
    io::{self, Read as _, Seek as _, SeekFrom},
    mem,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Mutex, MutexGuard},
    thread::{self, JoinHandle},
};

use process_wrap::tokio::{CommandWrap, KillOnDrop};

#[cfg(windows)]
use process_wrap::tokio::JobObject;

use tokio::{
    io::{AsyncBufReadExt as _, AsyncRead, AsyncReadExt as _, BufReader},
    process as asynchronous,
    runtime::Builder,
    sync::watch,
};

use tokio_util::sync::CancellationToken;

use super::{Error, Sourcemap, Workspace, process};
use crate::settings::resolution::Generator;

const DIAGNOSTIC_LIMIT: usize = 8 * 1024;
const SOURCE_LIMIT: usize = 64 * 1024 * 1024;

impl Workspace {
    /// Return configured and automatically discovered sourcemap generators.
    #[must_use]
    pub fn generators(&self) -> BTreeSet<Generator> {
        self.scopes
            .borrow()
            .values()
            .filter_map(|scope| scope.configuration.resolve.generator.as_ref())
            .filter(|generator| !generator.command.is_empty())
            .cloned()
            .chain(self.instances.borrow().discovered().cloned())
            .collect()
    }

    /// Obtain a validated sourcemap from the session or a one-shot command.
    ///
    /// # Errors
    /// Returns process failures, invalid sourcemaps, or cancellation.
    pub fn generate(&self, generator: &Generator) -> Result<Arc<Sourcemap>, Error> {
        let cancellation = self.cancellation.borrow().clone();
        let is_cancelled = &|| cancellation.is_cancelled();
        process::interrupted(is_cancelled)?;

        if let Some(generation) = &self.generation
            && !generator.watch.is_empty()
        {
            return generation.source(generator, &cancellation);
        }

        let mut command = command(generator)?;
        let executable = &generator.command[0];
        let failure = |source| failure(executable, source);
        let output = tempfile::NamedTempFile::new().map_err(failure)?;
        let diagnostics = tempfile::NamedTempFile::new().map_err(failure)?;

        command
            .command_mut()
            .stdout(output.reopen().map_err(failure)?)
            .stderr(diagnostics.reopen().map_err(failure)?);

        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;

        let status = runtime.block_on(async {
            let mut child = command.spawn().map_err(failure)?;

            process::wait(child.as_mut(), &cancellation, failure).await
        })?;

        if !status.success() {
            let mut file = fs::File::open(diagnostics.path()).map_err(failure)?;
            let length = file.metadata().map_err(failure)?.len();

            file.seek(SeekFrom::Start(
                length.saturating_sub(DIAGNOSTIC_LIMIT as u64),
            ))
            .map_err(failure)?;

            let mut diagnostic = Vec::new();

            file.take(DIAGNOSTIC_LIMIT as u64)
                .read_to_end(&mut diagnostic)
                .map_err(failure)?;

            return Err(failure(io::Error::other(format!(
                "Generator exited with {status}. {}",
                String::from_utf8_lossy(&diagnostic).trim()
            ))));
        }

        process::interrupted(is_cancelled)?;

        let mut source = String::new();

        fs::File::open(output.path())
            .map_err(failure)?
            .take((SOURCE_LIMIT + 1) as u64)
            .read_to_string(&mut source)
            .map_err(failure)?;

        parse(executable, &source)
    }

    /// Return cached and preseeded sources, including lazily discovered generators.
    #[must_use]
    pub fn generated(&self) -> BTreeMap<Generator, Arc<Sourcemap>> {
        self.instances.borrow().generated()
    }

    /// Seed generated sources and clear cached native instance maps.
    pub fn set_generated(&mut self, sources: BTreeMap<Generator, Arc<Sourcemap>>) {
        self.instances.get_mut().set_generated(sources);
    }
}

/// Session-owned sourcemap producers shared by worker-local workspaces.
#[derive(Clone)]
pub struct Generators {
    registry: Arc<Mutex<Registry>>,
}

impl Generators {
    /// Create a session with cancellation and a source-labeled message callback.
    #[must_use]
    pub fn new(cancellation: &CancellationToken, report: fn(&str, &str)) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry {
                streams: BTreeMap::new(),
                cancellation: cancellation.child_token(),
                report,
            })),
        }
    }

    /// Adopt changed snapshots between analysis batches.
    ///
    /// # Errors
    /// Returns cancellation.
    pub fn advance(&self) -> Result<bool, Error> {
        let mut registry = self.registry();
        registry.check()?;
        let mut has_changed = false;

        for (generator, stream) in &mut registry.streams {
            match stream.latest() {
                Ok(Some(source)) => {
                    has_changed |= stream.active.as_ref().is_none_or(|previous| {
                        !Arc::ptr_eq(previous, &source) && previous != &source
                    });

                    stream.active = Some(source);
                }

                Ok(None) => {}

                Err(error) => {
                    if !stream.failed {
                        tracing::warn!(directory = %generator.directory.display(), %error, "Sourcemap generation failed");
                        stream.failed = true;
                        has_changed = true;
                    }

                    stream.active = None;
                }
            }
        }

        Ok(has_changed)
    }

    /// Capture the active sourcemaps without adopting newer producer output.
    #[must_use]
    pub fn snapshots(&self) -> BTreeMap<Generator, Arc<Sourcemap>> {
        self.registry()
            .streams
            .iter()
            .filter_map(|(generator, stream)| {
                stream
                    .active
                    .as_ref()
                    .map(|source| (generator.clone(), Arc::clone(source)))
            })
            .collect()
    }

    /// Stop all producers and prevent this session from starting new ones.
    ///
    /// # Errors
    /// Returns process cleanup or worker failures after joining every producer.
    pub fn shutdown(&self) -> Result<(), Error> {
        let streams = {
            let mut registry = self.registry();
            registry.cancellation.cancel();

            mem::take(&mut registry.streams)
        };

        let mut failure = None;

        for (_, mut stream) in streams {
            if let Err(error) = stream.join()
                && failure.is_none()
            {
                failure = Some(error);
            }
        }

        failure.map_or(Ok(()), Err)
    }

    fn source(
        &self,
        generator: &Generator,
        cancellation: &CancellationToken,
    ) -> Result<Arc<Sourcemap>, Error> {
        loop {
            process::interrupted(&|| cancellation.is_cancelled())?;

            let (mut output, shutdown) = {
                let mut registry = self.registry();
                registry.check()?;

                let shutdown = registry.cancellation.clone();
                let report = registry.report;

                if let Entry::Vacant(entry) = registry.streams.entry(generator.clone()) {
                    entry.insert(Stream::new(generator, shutdown.clone(), report)?);
                }

                let stream = registry
                    .streams
                    .get_mut(generator)
                    .expect("Internal error: generator stream is missing.");

                let mut output = stream.output.clone();
                output.borrow_and_update();

                let latest = stream.latest()?;

                if let Some(source) = &stream.active {
                    return Ok(Arc::clone(source));
                }

                if let Some(source) = latest {
                    stream.active = Some(Arc::clone(&source));

                    return Ok(source);
                }

                (output, shutdown)
            };

            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Error::Runtime)?;

            runtime.block_on(async {
                tokio::select! {
                    biased;
                    () = cancellation.cancelled() => Err(Error::Cancelled),
                    () = shutdown.cancelled() => Err(Error::Cancelled),
                    result = output.changed() => result.map_err(|error| {
                        failure(&generator.command[0], io::Error::other(error))
                    }),
                }
            })?;
        }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("Internal error: generator registry lock is poisoned.")
    }
}

struct Registry {
    streams: BTreeMap<Generator, Stream>,
    cancellation: CancellationToken,
    report: fn(&str, &str),
}

impl Registry {
    fn check(&self) -> Result<(), Error> {
        process::interrupted(&|| self.cancellation.is_cancelled())
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.cancellation.cancel();

        for stream in self.streams.values_mut() {
            if let Err(error) = stream.join() {
                (self.report)(&stream.executable, &error.to_string());
            }
        }
    }
}

struct Stream {
    executable: String,
    active: Option<Arc<Sourcemap>>,
    output: watch::Receiver<Output>,
    thread: Option<JoinHandle<Result<(), Arc<Error>>>>,
    failed: bool,
}

impl Stream {
    fn new(
        generator: &Generator,
        cancellation: CancellationToken,
        report: fn(&str, &str),
    ) -> Result<Self, Error> {
        let mut command = command(generator)?;

        command
            .command_mut()
            .args(&generator.watch)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;

        let executable = generator.command[0].clone();
        let program = executable.clone();
        let (sender, output) = watch::channel(Output::Pending);

        let thread = thread::Builder::new()
            .spawn(move || {
                runtime.block_on(supervise(command, &program, &sender, &cancellation, report))
            })
            .map_err(|source| failure(&executable, source))?;

        Ok(Self {
            executable,
            active: None,
            output,
            thread: Some(thread),
            failed: false,
        })
    }

    fn latest(&self) -> Result<Option<Arc<Sourcemap>>, Error> {
        let is_finished = self.thread.as_ref().is_none_or(JoinHandle::is_finished);

        let source = match self.output.borrow().clone() {
            Output::Pending => None,
            Output::Snapshot(source) => Some(source),
            Output::Failed(error) => return Err(Error::Generation(error)),
        };

        if is_finished {
            return Err(failure(
                &self.executable,
                io::Error::other("Watch generator stopped."),
            ));
        }

        Ok(source)
    }

    fn join(&mut self) -> Result<(), Error> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };

        thread
            .join()
            .map_err(|panic| {
                let reason = panic
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("Unknown panic payload.");

                failure(
                    &self.executable,
                    io::Error::other(format!("Generator worker panicked. {reason}")),
                )
            })?
            .map_err(Error::Generation)
    }
}

#[derive(Clone)]
enum Output {
    Pending,
    Snapshot(Arc<Sourcemap>),
    Failed(Arc<Error>),
}

async fn supervise(
    mut command: CommandWrap,
    executable: &str,
    output: &watch::Sender<Output>,
    cancellation: &CancellationToken,
    report: fn(&str, &str),
) -> Result<(), Arc<Error>> {
    let child = command.spawn();

    let mut child = match child {
        Ok(child) => child,

        Err(source) => {
            output.send_replace(Output::Failed(Arc::new(failure(executable, source))));

            return Ok(());
        }
    };

    let stdout = child
        .stdout()
        .take()
        .expect("Internal error: generator standard output is not piped.");

    let stderr = child
        .stderr()
        .take()
        .expect("Internal error: generator standard error is not piped.");

    let diagnostics = messages(stderr, executable, report);
    tokio::pin!(diagnostics);
    let snapshots = snapshots(stdout, executable, output);
    tokio::pin!(snapshots);
    let mut is_drained = false;
    let mut diagnostic = String::new();

    let result = loop {
        tokio::select! {
            biased;

            () = cancellation.cancelled() => break Err(Error::Cancelled),
            status = child.inner_mut().wait() => {
                break match status {
                    Ok(status) => Err(failure(executable, io::Error::other(format!(
                        "Watch generator exited with {status}."
                    )))),
                    Err(source) => Err(failure(executable, source)),
                };
            },
            result = &mut snapshots => break result,
            result = &mut diagnostics, if !is_drained => {
                is_drained = true;

                match result {
                    Ok(message) => diagnostic = message,
                    Err(source) => break Err(failure(executable, source)),
                }
            },
        }
    };

    let cleanup = process::stop(child.as_mut()).await;

    if let Err(source) = cleanup {
        let error = Arc::new(failure(executable, source));
        output.send_replace(Output::Failed(Arc::clone(&error)));

        return Err(error);
    }

    if !is_drained {
        match diagnostics.await {
            Ok(message) => diagnostic = message,

            Err(source) => {
                output.send_replace(Output::Failed(Arc::new(failure(executable, source))));

                return Ok(());
            }
        }
    }

    if let Err(mut error) = result
        && !matches!(error, Error::Cancelled)
    {
        if !diagnostic.is_empty()
            && let Error::Generate { details, .. } = &mut error
        {
            *details = format!("\n{}", diagnostic.trim_end());
        }

        output.send_replace(Output::Failed(Arc::new(error)));
    }

    Ok(())
}

async fn messages(
    stderr: impl AsyncRead + Unpin,
    executable: &str,
    report: fn(&str, &str),
) -> io::Result<String> {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    let mut diagnostic = Vec::new();

    loop {
        let contents = reader.fill_buf().await?;

        let count = contents
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(contents.len(), |index| index + 1);

        let is_complete = count != 0 && contents[count - 1] == b'\n';
        retain(&mut line, &contents[..count]);
        reader.consume(count);

        if is_complete || (count == 0 && !line.is_empty()) {
            if line.last() == Some(&b'\n') {
                line.pop();

                if line.last() == Some(&b'\r') {
                    line.pop();
                }
            }

            report(executable, &String::from_utf8_lossy(&line));
            retain(&mut diagnostic, &line);
            retain(&mut diagnostic, b"\n");
            line.clear();
        }

        if count == 0 {
            break;
        }
    }

    Ok(String::from_utf8_lossy(&diagnostic).into_owned())
}

fn retain(output: &mut Vec<u8>, contents: &[u8]) {
    output.extend_from_slice(&contents[contents.len().saturating_sub(DIAGNOSTIC_LIMIT)..]);
    output.drain(..output.len().saturating_sub(DIAGNOSTIC_LIMIT));
}

async fn snapshots(
    stdout: impl AsyncRead + Unpin,
    executable: &str,
    output: &watch::Sender<Output>,
) -> Result<(), Error> {
    let mut reader = BufReader::new(stdout);

    loop {
        let mut line = String::new();

        let length = (&mut reader)
            .take((SOURCE_LIMIT + 1) as u64)
            .read_line(&mut line)
            .await
            .map_err(|source| failure(executable, source))?;

        if length == 0 {
            break;
        }

        if length > SOURCE_LIMIT {
            return Err(failure(
                executable,
                io::Error::other("Sourcemap exceeds 64 MiB."),
            ));
        }

        if line.ends_with('\n') {
            line.pop();

            if line.ends_with('\r') {
                line.pop();
            }
        }

        if !line.trim().is_empty() {
            output.send_replace(Output::Snapshot(parse(executable, &line)?));
        }
    }

    Err(failure(
        executable,
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "Watch generator closed stdout.",
        ),
    ))
}

fn command(generator: &Generator) -> Result<CommandWrap, Error> {
    let executable = generator.command.first().map_or("", String::as_str);

    if executable.trim().is_empty() {
        return Err(failure(
            executable,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Expected a nonempty executable.",
            ),
        ));
    }

    let program = if Path::new(executable).components().count() > 1 {
        generator.directory.join(executable)
    } else {
        PathBuf::from(executable)
    };

    let mut command = asynchronous::Command::new(program);

    command
        .args(&generator.command[1..])
        .current_dir(&generator.directory)
        .stdin(Stdio::null());

    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);

    #[cfg(windows)]
    command.wrap(JobObject);

    Ok(command)
}

fn parse(executable: &str, source: &str) -> Result<Arc<Sourcemap>, Error> {
    if source.len() > SOURCE_LIMIT {
        return Err(failure(
            executable,
            io::Error::other("Sourcemap exceeds 64 MiB."),
        ));
    }

    serde_json::from_str(source)
        .map(Arc::new)
        .map_err(|source| Error::GeneratedData {
            executable: executable.to_owned(),
            source,
        })
}

fn failure(executable: &str, source: io::Error) -> Error {
    Error::Generate {
        executable: executable.to_owned(),
        source,
        details: String::new(),
    }
}
