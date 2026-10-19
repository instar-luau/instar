use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write as _},
    path::{Path, PathBuf},
    rc::Rc,
    thread,
    time::Duration,
};

use reqwest::Client;
use tokio::{runtime::Builder, sync::oneshot, time};

use super::{Error, Workspace, environment, process};
use crate::assets::digest;

const ADDRESS: &str = "https://instar-luau.github.io/instar";

#[derive(Copy, Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) enum Namespace {
    Luau,
    Roblox,
}

impl Namespace {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Luau => "luau",
            Self::Roblox => "roblox",
        }
    }

    fn files(self) -> &'static [&'static str] {
        match self {
            Self::Luau => &["documentation.json"],

            Self::Roblox => &[
                "documentation.json",
                "none.d.luau",
                "local.d.luau",
                "plugin.d.luau",
                "roblox.d.luau",
            ],
        }
    }
}

pub(super) struct Refresh {
    cancellation: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Drop for Refresh {
    fn drop(&mut self) {
        drop(self.cancellation.take());

        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("asset refresh worker panicked");
        }
    }
}

pub(super) fn directory(
    workspace: &Workspace,
    namespace: Namespace,
    refreshes: &mut Vec<Refresh>,
) -> Result<PathBuf, Error> {
    let cancellation = Rc::clone(&workspace.cancelled.borrow());
    let cancelled = cancellation.as_ref();
    process::interrupted(cancelled)?;

    let parent = dirs::cache_dir()
        .ok_or_else(|| Error::Path {
            path: workspace.root.clone(),
            reason: "the operating system cache directory is unavailable",
        })?
        .join("instar")
        .join(namespace.name());

    fs::create_dir_all(&parent).map_err(|source| filesystem(&parent, source))?;

    let runtime = Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(Error::Runtime)?;

    runtime.block_on(async {
        tokio::select! {
            biased;

            () = async {
                while !cancelled() {
                    time::sleep(Duration::from_millis(50)).await;
                }
            } => Err(Error::Cancelled),
            result = acquire(&parent, namespace, refreshes, cancelled) => result,
        }
    })
}

async fn acquire(
    parent: &Path,
    namespace: Namespace,
    refreshes: &mut Vec<Refresh>,
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, Error> {
    if let Some(directory) = cached(parent)? {
        validate(&directory, namespace, cancelled)?;
        process::interrupted(cancelled)?;

        match update(parent.to_owned(), namespace) {
            Ok(refresh) => refreshes.push(refresh),

            Err(error) => {
                tracing::warn!(%error, namespace = namespace.name(), "cannot start asset refresh");
            }
        }

        return Ok(directory);
    }

    let lock_path = parent.join("lock");
    let lock = lockfile(&lock_path)?;

    loop {
        process::interrupted(cancelled)?;

        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => time::sleep(Duration::from_millis(50)).await,
            Err(TryLockError::Error(source)) => return Err(filesystem(&lock_path, source)),
        }
    }

    if let Some(directory) = cached(parent)? {
        validate(&directory, namespace, cancelled)?;

        return Ok(directory);
    }

    refresh(parent, namespace, None, cancelled).await
}

fn update(parent: PathBuf, namespace: Namespace) -> io::Result<Refresh> {
    let (cancellation, cancelled) = oneshot::channel();

    let thread = thread::Builder::new()
        .name(format!("{} assets", namespace.name()))
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
                            result = async {
                                let lock_path = parent.join("lock");
                                let lock = lockfile(&lock_path)?;

                                match lock.try_lock() {
                                    Ok(()) => {}
                                    Err(TryLockError::WouldBlock) => return Ok(()),
                                    Err(TryLockError::Error(source)) => {
                                        return Err(filesystem(&lock_path, source));
                                    }
                                }

                                let cached = cached(&parent)?;
                                refresh(&parent, namespace, cached.as_deref(), &|| false).await?;

                                Ok(())
                            } => result,
                        }
                    })
                });

            if let Err(error) = result {
                tracing::warn!(%error, namespace = namespace.name(), "cannot refresh cached assets");
            }
        })?;

    Ok(Refresh {
        cancellation: Some(cancellation),
        thread: Some(thread),
    })
}

fn lockfile(path: &Path) -> Result<File, Error> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|source| filesystem(path, source))
}

async fn refresh(
    parent: &Path,
    namespace: Namespace,
    cached: Option<&Path>,
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, Error> {
    let staging = tempfile::tempdir_in(parent).map_err(|source| filesystem(parent, source))?;
    let bundle = staging.path().join("bundle");
    fs::create_dir(&bundle).map_err(|source| filesystem(&bundle, source))?;

    let address = format!("{ADDRESS}/{}", namespace.name());

    let client = Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|source| Error::Download {
            url: address.clone(),
            source,
        })?;

    let metadata = bundle.join("metadata.json");
    download(&client, &address, "metadata.json", &metadata, cancelled).await?;
    let contents = fs::read(&metadata).map_err(|source| filesystem(&metadata, source))?;
    let identity = digest(&contents);
    let directory = parent.join(&identity);

    if exists(&directory)? {
        validate(&directory, namespace, cancelled)?;
    } else {
        for filename in namespace.files() {
            download(
                &client,
                &address,
                filename,
                &bundle.join(filename),
                cancelled,
            )
            .await?;
        }

        environment::validate(&bundle, namespace, cancelled)?;
        process::interrupted(cancelled)?;
        fs::rename(&bundle, &directory).map_err(|source| filesystem(&directory, source))?;
    }

    process::interrupted(cancelled)?;

    if cached != Some(directory.as_path()) {
        let pointer = parent.join("current");

        let mut staged =
            tempfile::NamedTempFile::new_in(parent).map_err(|source| filesystem(parent, source))?;

        staged
            .write_all(identity.as_bytes())
            .map_err(|source| filesystem(&pointer, source))?;

        staged
            .as_file()
            .sync_all()
            .map_err(|source| filesystem(&pointer, source))?;

        process::interrupted(cancelled)?;

        staged
            .persist(&pointer)
            .map_err(|error| filesystem(&pointer, error.error))?;
    }

    Ok(directory)
}

async fn download(
    client: &Client,
    address: &str,
    filename: &str,
    destination: &Path,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    process::interrupted(cancelled)?;

    let url = format!("{address}/{filename}");

    let failure = |source| Error::Download {
        url: url.clone(),
        source,
    };

    let response = client.get(&url).send().await.map_err(failure)?;
    let mut response = response.error_for_status().map_err(failure)?;
    let mut file = File::create(destination).map_err(|source| filesystem(destination, source))?;

    while let Some(chunk) = response.chunk().await.map_err(failure)? {
        process::interrupted(cancelled)?;

        file.write_all(&chunk)
            .map_err(|source| filesystem(destination, source))?;
    }

    Ok(())
}

fn cached(parent: &Path) -> Result<Option<PathBuf>, Error> {
    let pointer = parent.join("current");

    let identity = match fs::read_to_string(&pointer) {
        Ok(identity) => identity,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(filesystem(&pointer, source)),
    };

    if identity.len() != 64 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::Path {
            path: pointer,
            reason: "invalid cached asset identity",
        });
    }

    let directory = parent.join(identity);

    Ok(exists(&directory)?.then_some(directory))
}

fn validate(
    directory: &Path,
    namespace: Namespace,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), Error> {
    process::interrupted(cancelled)?;
    let metadata = directory.join("metadata.json");
    let contents = fs::read(&metadata).map_err(|source| filesystem(&metadata, source))?;

    if directory.file_name().and_then(|name| name.to_str()) != Some(digest(&contents).as_str()) {
        return Err(Error::Path {
            path: metadata,
            reason: "cached asset metadata does not match its identity",
        });
    }

    environment::validate(directory, namespace, cancelled)?;

    Ok(())
}

fn exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(filesystem(path, source)),
    }
}

fn filesystem(path: &Path, source: io::Error) -> Error {
    Error::Filesystem {
        path: path.to_owned(),
        source,
    }
}
