use std::{
    fmt::Write as _,
    fs::{self, File, OpenOptions, TryLockError},
    io::{self, Write as _},
    path::{Path, PathBuf},
    time::Duration,
};

use reqwest::Client;
use sha2::{Digest as _, Sha256};
use tokio::{runtime::Builder, time};

use super::{Error, Workspace, environment, process};

const ADDRESS: &str = "https://instar-luau.github.io/instar/roblox";
const FILES: [&str; 6] = [
    "documentation.json",
    "enumerations.d.luau",
    "none.d.luau",
    "local.d.luau",
    "plugin.d.luau",
    "roblox.d.luau",
];

pub(super) fn directory(workspace: &Workspace) -> Result<PathBuf, Error> {
    let cancelled = workspace.cancelled;
    process::interrupted(cancelled)?;

    let parent = dirs::cache_dir()
        .ok_or_else(|| Error::Path {
            path: workspace.root.clone(),
            reason: "the operating system cache directory is unavailable",
        })?
        .join("instar")
        .join("roblox");

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
            result = acquire(&parent, cancelled) => result,
        }
    })
}

async fn acquire(parent: &Path, cancelled: fn() -> bool) -> Result<PathBuf, Error> {
    let lock_path = parent.join("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| filesystem(&lock_path, source))?;

    loop {
        process::interrupted(cancelled)?;

        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) => time::sleep(Duration::from_millis(50)).await,
            Err(TryLockError::Error(source)) => return Err(filesystem(&lock_path, source)),
        }
    }

    let cached = cached(parent)?;
    let staging = tempfile::tempdir_in(parent).map_err(|source| filesystem(parent, source))?;
    let bundle = staging.path().join("bundle");
    fs::create_dir(&bundle).map_err(|source| filesystem(&bundle, source))?;

    let client = Client::builder()
        .https_only(true)
        .connect_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|source| Error::Download {
            url: ADDRESS.to_owned(),
            source,
        })?;
    let metadata = bundle.join("metadata.json");

    if let Err(error) = download(&client, "metadata.json", &metadata, cancelled).await {
        if matches!(error, Error::Download { .. })
            && let Some(directory) = cached
        {
            process::interrupted(cancelled)?;
            tracing::warn!(%error, "cannot refresh Roblox assets; using cached bundle");
            return Ok(directory);
        }

        return Err(error);
    }

    let contents = fs::read(&metadata).map_err(|source| filesystem(&metadata, source))?;
    let mut identity = String::with_capacity(64);

    for byte in Sha256::digest(&contents) {
        write!(identity, "{byte:02x}").expect("internal error: cannot write to a string");
    }

    let directory = parent.join(&identity);

    if !exists(&directory)? {
        for filename in FILES {
            download(&client, filename, &bundle.join(filename), cancelled).await?;
        }

        environment::validate(&bundle, cancelled)?;
        process::interrupted(cancelled)?;
        fs::rename(&bundle, &directory).map_err(|source| filesystem(&directory, source))?;
    }

    process::interrupted(cancelled)?;

    if cached.as_ref() != Some(&directory) {
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
        staged
            .persist(&pointer)
            .map_err(|error| filesystem(&pointer, error.error))?;
    }

    Ok(directory)
}

async fn download(
    client: &Client,
    filename: &str,
    destination: &Path,
    cancelled: fn() -> bool,
) -> Result<(), Error> {
    process::interrupted(cancelled)?;

    let url = format!("{ADDRESS}/{filename}");
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
