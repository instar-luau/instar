use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
    thread,
    time::{Duration, SystemTime},
};

use reqwest::Client;
use tokio::{runtime::Builder, sync::oneshot};
use tokio_util::sync::CancellationToken;

use super::Error;

const ADDRESS: &str = "https://instar-luau.github.io/instar";

static SHARED: Mutex<Weak<Shared>> = Mutex::new(Weak::new());

type Files = Mutex<BTreeMap<(Namespace, &'static str), Cached>>;

pub(crate) struct Cache(Arc<Shared>);

struct Shared {
    files: Arc<Files>,
    refreshes: Mutex<BTreeMap<(Namespace, &'static str), Refresh>>,
}

struct Cached {
    source: Arc<str>,
    checked: SystemTime,
}

impl Default for Cache {
    fn default() -> Self {
        let mut shared = SHARED
            .lock()
            .expect("Internal error: asset cache mutex is poisoned.");

        let assets = shared.upgrade().unwrap_or_else(|| {
            let assets = Arc::new(Shared {
                files: Arc::default(),
                refreshes: Mutex::default(),
            });

            *shared = Arc::downgrade(&assets);

            assets
        });

        Self(assets)
    }
}

impl Cache {
    pub(crate) fn load(
        &self,
        namespace: Namespace,
        filename: &'static str,
        cancellation: &CancellationToken,
    ) -> Result<(PathBuf, Arc<str>), Error> {
        let path = directory()?.join(namespace.name()).join(filename);
        let source = acquire(&path, (namespace, filename), &self.0, cancellation)?;

        Ok((path, source))
    }
}

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Namespace {
    Luau,
    Roblox,
}

impl Namespace {
    fn name(self) -> &'static str {
        match self {
            Self::Luau => "luau",
            Self::Roblox => "roblox",
        }
    }
}

struct Refresh {
    cancellation: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Refresh {
    fn drop(&mut self) {
        drop(self.cancellation.take());

        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("Asset refresh worker panicked.");
        }
    }
}

/// Download fresh published environment assets into the disk cache.
///
/// # Errors
/// Returns cache path, runtime, download, filesystem, or cancellation failures.
pub fn refresh(cancellation: &CancellationToken) -> Result<Vec<PathBuf>, Error> {
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let directory = directory()?;

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;

    runtime.block_on(async {
        let files = Mutex::new(BTreeMap::new());
        let mut refreshed = Vec::new();

        for key in [
            (Namespace::Luau, "documentation.json"),
            (Namespace::Roblox, "documentation.json"),
            (Namespace::Roblox, "none.d.luau"),
            (Namespace::Roblox, "local.d.luau"),
            (Namespace::Roblox, "plugin.d.luau"),
            (Namespace::Roblox, "roblox.d.luau"),
            (Namespace::Roblox, "metadata.json"),
        ] {
            let path = directory.join(key.0.name()).join(key.1);

            tokio::select! {
                biased;
                () = cancellation.cancelled() => return Err(Error::Cancelled),
                result = download(&files, &path, key) => { result?; },
            }

            refreshed.push(path);
        }

        Ok(refreshed)
    })
}

fn directory() -> Result<PathBuf, Error> {
    dirs::cache_dir()
        .map(|path| path.join("instar"))
        .ok_or_else(|| {
            filesystem(
                Path::new("instar"),
                io::Error::new(
                    io::ErrorKind::NotFound,
                    "The operating system cache directory is unavailable.",
                ),
            )
        })
}

fn acquire(
    path: &Path,
    key: (Namespace, &'static str),
    shared: &Shared,
    cancellation: &CancellationToken,
) -> Result<Arc<str>, Error> {
    if cancellation.is_cancelled() {
        return Err(Error::Cancelled);
    }

    let mut files = shared
        .files
        .lock()
        .expect("Internal error: asset cache mutex is poisoned.");

    if !files.contains_key(&key)
        && let Some(cached) = read(path)?
    {
        files.insert(key, cached);
    }

    if let Some(cached) = files.get(&key) {
        let source = Arc::clone(&cached.source);

        let expired = cached
            .checked
            .elapsed()
            .map_or(true, |age| age >= Duration::from_hours(12));

        drop(files);

        if expired {
            let mut refreshes = shared
                .refreshes
                .lock()
                .expect("Internal error: asset refresh mutex is poisoned.");

            if refreshes.get(&key).is_none_or(|refresh| {
                refresh
                    .thread
                    .as_ref()
                    .is_none_or(thread::JoinHandle::is_finished)
            }) {
                match update(path.to_owned(), key, Arc::clone(&shared.files)) {
                    Ok(refresh) => {
                        refreshes.insert(key, refresh);
                    }

                    Err(error) => tracing::warn!(%error, "Could not start asset refresh."),
                }
            }
        }

        return Ok(source);
    }

    drop(files);

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;

    runtime.block_on(async {
        tokio::select! {
            biased;
            () = cancellation.cancelled() => Err(Error::Cancelled),
            result = download(&shared.files, path, key) => result,
        }
    })
}

fn update(path: PathBuf, key: (Namespace, &'static str), files: Arc<Files>) -> io::Result<Refresh> {
    let (cancellation, cancelled) = oneshot::channel();

    let thread = thread::Builder::new()
        .name(format!("{} {}", key.0.name(), key.1))
        .spawn(move || {
            let result = Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(Error::Runtime)
                .and_then(|runtime| {
                    runtime.block_on(async {
                        tokio::select! {
                            biased;
                            _ = cancelled => Ok(()),
                            result = download(&files, &path, key) => result.map(|_| ()),
                        }
                    })
                });

            if let Err(error) = result {
                tracing::warn!(%error, "Could not refresh cached asset.");
            }
        })?;

    Ok(Refresh {
        cancellation: Some(cancellation),
        thread: Some(thread),
    })
}

async fn download(
    files: &Files,
    path: &Path,
    key: (Namespace, &'static str),
) -> Result<Arc<str>, Error> {
    let parent = path
        .parent()
        .expect("Internal error: asset has no parent directory.");

    fs::create_dir_all(parent).map_err(|source| filesystem(parent, source))?;

    let mut staged =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| filesystem(parent, source))?;

    let url = format!("{ADDRESS}/{}/{}", key.0.name(), key.1);

    let failure = |source| Error::Download {
        address: url.clone(),
        source,
    };

    let client = Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(failure)?;

    let mut response = client
        .get(&url)
        .send()
        .await
        .map_err(failure)?
        .error_for_status()
        .map_err(failure)?;

    while let Some(chunk) = response.chunk().await.map_err(failure)? {
        staged
            .write_all(&chunk)
            .map_err(|source| filesystem(path, source))?;
    }

    let source = fs::read_to_string(staged.path()).map_err(|source| filesystem(path, source))?;

    let mut files = files
        .lock()
        .expect("Internal error: asset cache mutex is poisoned.");

    let source = files
        .get(&key)
        .filter(|cached| *cached.source == source)
        .map_or_else(|| Arc::from(source), |cached| Arc::clone(&cached.source));

    let checked = SystemTime::now();

    staged
        .as_file()
        .set_modified(checked)
        .map_err(|source| filesystem(path, source))?;

    staged
        .persist(path)
        .map_err(|error| filesystem(path, error.error))?;

    files.insert(
        key,
        Cached {
            source: Arc::clone(&source),
            checked,
        },
    );

    Ok(source)
}

fn read(path: &Path) -> Result<Option<Cached>, Error> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(filesystem(path, source)),
    };

    let checked = file
        .metadata()
        .and_then(|metadata| metadata.modified())
        .map_err(|source| filesystem(path, source))?;

    let source = io::read_to_string(file).map_err(|source| filesystem(path, source))?;

    Ok(Some(Cached {
        source: source.into(),
        checked,
    }))
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    Error::Filesystem {
        path: path.to_owned(),
        source,
    }
}
