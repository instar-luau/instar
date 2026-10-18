use std::{
    collections::{BTreeMap, BTreeSet, btree_map::Entry},
    fs, io, mem,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt as _, BufReader},
    process as asynchronous,
    runtime::Builder,
    sync::watch,
    time,
};

use super::{Error, Workspace, instances, process};
use crate::settings::resolution::Generator;

impl Workspace {
    /// Return nonempty generators from all known configuration scopes.
    #[must_use]
    pub fn generators(&self) -> BTreeSet<Generator> {
        self.scopes
            .borrow()
            .values()
            .filter_map(|scope| scope.configuration.resolve.generator.as_ref())
            .filter(|generator| !generator.command.is_empty())
            .cloned()
            .collect()
    }

    /// Obtain a validated sourcemap from the session or a one-shot command.
    ///
    /// # Errors
    /// Returns process failures, invalid sourcemaps, or cancellation.
    pub fn generate(&self, generator: &Generator) -> Result<Arc<str>, Error> {
        process::interrupted(self.cancelled)?;

        if let Some(generation) = &self.generation {
            return generation.source(generator);
        }

        let mut command = command(generator)?;
        let executable = &generator.command[0];
        let failure = |source| failure(executable, source);
        let output = tempfile::NamedTempFile::new().map_err(failure)?;
        let diagnostics = tempfile::NamedTempFile::new().map_err(failure)?;

        let mut child = command
            .stdout(output.reopen().map_err(failure)?)
            .stderr(diagnostics.reopen().map_err(failure)?)
            .spawn()
            .map_err(failure)?;

        let status = process::wait(&mut child, self.cancelled, failure)?;

        if !status.success() {
            let diagnostic = fs::read(diagnostics.path()).map_err(failure)?;

            return Err(failure(io::Error::other(format!(
                "exited with {status}: {}",
                String::from_utf8_lossy(&diagnostic).trim()
            ))));
        }

        process::interrupted(self.cancelled)?;

        validate(
            executable,
            fs::read_to_string(output.path()).map_err(failure)?,
        )
    }

    /// Return cached and preseeded sources, including lazily discovered generators.
    #[must_use]
    pub fn generated(&self) -> BTreeMap<Generator, Arc<str>> {
        self.instances.borrow().generated()
    }

    /// Seed generated sources and clear cached native instance maps.
    pub fn set_generated(&mut self, sources: BTreeMap<Generator, Arc<str>>) {
        self.instances.get_mut().set_generated(sources);
    }
}

/// Session-owned sourcemap producers shared by worker-local workspaces.
#[derive(Clone)]
pub struct Generators {
    registry: Arc<Mutex<Registry>>,
}

impl Generators {
    /// Create a session that observes the supplied cancellation callback.
    #[must_use]
    pub fn new(cancelled: fn() -> bool) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Registry {
                streams: BTreeMap::new(),
                stopped: Arc::new(AtomicBool::new(false)),
                cancelled,
            })),
        }
    }

    /// Adopt changed snapshots between analysis batches.
    ///
    /// # Errors
    /// Returns producer failures or cancellation.
    pub fn advance(&self) -> Result<bool, Error> {
        let mut registry = self.registry();
        registry.check()?;
        let mut changed = false;

        for stream in registry.streams.values_mut() {
            if let Some(source) = stream.latest()? {
                changed |= stream
                    .active
                    .as_ref()
                    .is_none_or(|previous| !Arc::ptr_eq(previous, &source) && previous != &source);

                stream.active = Some(source);
            }
        }

        Ok(changed)
    }

    /// Stop all producers and prevent this session from starting new ones.
    ///
    /// # Errors
    /// Returns process cleanup or worker failures after joining every producer.
    pub fn shutdown(&self) -> Result<(), Error> {
        let streams = {
            let mut registry = self.registry();
            registry.stopped.store(true, Ordering::Relaxed);

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

    fn source(&self, generator: &Generator) -> Result<Arc<str>, Error> {
        loop {
            {
                let mut registry = self.registry();
                registry.check()?;

                for stream in registry.streams.values() {
                    stream.latest()?;
                }

                let stopped = Arc::clone(&registry.stopped);
                let cancelled = registry.cancelled;

                let stream = match registry.streams.entry(generator.clone()) {
                    Entry::Occupied(entry) => entry.into_mut(),

                    Entry::Vacant(entry) => {
                        entry.insert(Stream::new(generator, stopped, cancelled)?)
                    }
                };

                let latest = stream.latest()?;

                if let Some(source) = &stream.active {
                    return Ok(Arc::clone(source));
                }

                if let Some(source) = latest {
                    stream.active = Some(Arc::clone(&source));

                    return Ok(source);
                }
            }

            thread::sleep(Duration::from_millis(50));
        }
    }

    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry
            .lock()
            .expect("internal error: generator registry poisoned")
    }
}

struct Registry {
    streams: BTreeMap<Generator, Stream>,
    stopped: Arc<AtomicBool>,
    cancelled: fn() -> bool,
}

impl Registry {
    fn check(&self) -> Result<(), Error> {
        process::interrupted(self.cancelled)?;

        if self.stopped.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }

        Ok(())
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);

        for stream in self.streams.values_mut() {
            if let Err(error) = stream.join() {
                eprintln!("{error}");
            }
        }
    }
}

struct Stream {
    executable: String,
    active: Option<Arc<str>>,
    output: watch::Receiver<Output>,
    thread: Option<JoinHandle<Result<(), Arc<Error>>>>,
}

impl Stream {
    fn new(
        generator: &Generator,
        stopped: Arc<AtomicBool>,
        cancelled: fn() -> bool,
    ) -> Result<Self, Error> {
        let mut command = command(generator)?;

        command
            .args(&generator.watch)
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());

        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;

        let executable = generator.command[0].clone();
        let program = executable.clone();
        let (sender, output) = watch::channel(Output::Pending);

        let thread = thread::Builder::new()
            .spawn(move || {
                runtime.block_on(supervise(command, &program, &sender, &stopped, cancelled))
            })
            .map_err(|source| failure(&executable, source))?;

        Ok(Self {
            executable,
            active: None,
            output,
            thread: Some(thread),
        })
    }

    fn latest(&self) -> Result<Option<Arc<str>>, Error> {
        let finished = self.thread.as_ref().is_none_or(JoinHandle::is_finished);

        let source = match self.output.borrow().clone() {
            Output::Pending => None,
            Output::Snapshot(source) => Some(source),
            Output::Failed(error) => return Err(Error::Generation(error)),
        };

        if finished {
            return Err(failure(
                &self.executable,
                io::Error::other("watch generator stopped"),
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
                    .unwrap_or("unknown panic payload");

                failure(
                    &self.executable,
                    io::Error::other(format!("generator worker panicked: {reason}")),
                )
            })?
            .map_err(Error::Generation)
    }
}

#[derive(Clone)]
enum Output {
    Pending,
    Snapshot(Arc<str>),
    Failed(Arc<Error>),
}

async fn supervise(
    command: Command,
    executable: &str,
    output: &watch::Sender<Output>,
    stopped: &AtomicBool,
    cancelled: fn() -> bool,
) -> Result<(), Arc<Error>> {
    let mut command = asynchronous::Command::from(command);
    let child = command.kill_on_drop(true).spawn();

    let mut child = match child {
        Ok(child) => child,

        Err(source) => {
            output.send_replace(Output::Failed(Arc::new(failure(executable, source))));

            return Ok(());
        }
    };

    let stdout = child
        .stdout
        .take()
        .expect("internal error: generator stdout is not piped");

    let result = tokio::select! {
        biased;

        () = async {
            while !stopped.load(Ordering::Relaxed) && !cancelled() {
                time::sleep(Duration::from_millis(50)).await;
            }
        } => Err(Error::Cancelled),
        status = child.wait() => {
            match status {
                Ok(status) => Err(failure(executable, io::Error::other(format!(
                    "watch generator exited with {status}"
                )))),
                Err(source) => Err(failure(executable, source)),
            }
        },
        result = snapshots(stdout, executable, output) => result,
    };

    let cleanup = async {
        if child.try_wait()?.is_none()
            && let Err(source) = child.start_kill()
            && child.try_wait()?.is_none()
        {
            return Err(source);
        }

        child.wait().await?;

        Ok::<_, io::Error>(())
    }
    .await;

    if let Err(source) = cleanup {
        let error = Arc::new(failure(executable, source));
        output.send_replace(Output::Failed(Arc::clone(&error)));

        return Err(error);
    }

    if let Err(error) = result
        && !matches!(error, Error::Cancelled)
    {
        output.send_replace(Output::Failed(Arc::new(error)));
    }

    Ok(())
}

async fn snapshots(
    stdout: asynchronous::ChildStdout,
    executable: &str,
    output: &watch::Sender<Output>,
) -> Result<(), Error> {
    let mut lines = BufReader::new(stdout).lines();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|source| failure(executable, source))?
    {
        if !line.trim().is_empty() {
            output.send_replace(Output::Snapshot(validate(executable, line)?));
        }
    }

    Err(failure(
        executable,
        io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "watch generator closed stdout",
        ),
    ))
}

fn command(generator: &Generator) -> Result<Command, Error> {
    let executable = generator.command.first().map_or("", String::as_str);

    if executable.trim().is_empty() {
        return Err(failure(
            executable,
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a nonempty executable",
            ),
        ));
    }

    let program = if Path::new(executable).components().count() > 1 {
        generator.directory.join(executable)
    } else {
        PathBuf::from(executable)
    };

    let mut command = Command::new(program);

    command
        .args(&generator.command[1..])
        .current_dir(&generator.directory)
        .stdin(Stdio::null());

    Ok(command)
}

fn validate(executable: &str, source: String) -> Result<Arc<str>, Error> {
    instances::validate(&source).map_err(|source| Error::GeneratedData {
        executable: executable.to_owned(),
        source,
    })?;

    Ok(source.into())
}

fn failure(executable: &str, source: io::Error) -> Error {
    Error::Generate {
        executable: executable.to_owned(),
        source,
    }
}
