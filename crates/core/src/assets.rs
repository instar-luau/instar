use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use instar_analysis::{Options, error::invalid};
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const HOST: &str = "https://instar-luau.github.io/instar";

pub(crate) fn directory(namespace: &str) -> io::Result<PathBuf> {
    let base = dirs::cache_dir().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "user cache directory is unavailable",
        )
    })?;

    if !base.is_absolute() {
        return Err(invalid("user cache directory must be absolute"));
    }

    Ok(base.join("instar").join(namespace).join(VERSION))
}

pub(crate) fn parse<'source, T: Deserialize<'source>>(source: &'source str) -> io::Result<T> {
    let mut deserializer = serde_json::Deserializer::from_str(source);
    let value = serde_path_to_error::deserialize(&mut deserializer).map_err(invalid)?;
    deserializer.end().map_err(invalid)?;

    Ok(value)
}

pub(crate) fn verify(source: &str, expected: &str) -> io::Result<()> {
    if format!("{:x}", Sha256::digest(source.as_bytes())) != expected {
        return Err(invalid("assets do not match their metadata"));
    }

    Ok(())
}

pub(crate) fn fetch(
    url: String,
    options: &Options,
    download: fn(&str, &Options) -> io::Result<BTreeMap<String, String>>,
) -> io::Result<BTreeMap<String, String>> {
    let started = Instant::now();
    options.check(started)?;
    tracing::debug!(%url, "Downloading asset bundle");
    let (sender, receiver) = mpsc::sync_channel(1);
    let limits = options.clone();

    thread::Builder::new()
        .name("assets".to_owned())
        .spawn(move || {
            drop(sender.send(download(&url, &limits)));
        })?;

    loop {
        options.check(started)?;

        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => {
                tracing::debug!(
                    elapsed_seconds = started.elapsed().as_secs_f64(),
                    success = result.is_ok(),
                    "Asset download completed"
                );

                return result;
            }

            Err(mpsc::RecvTimeoutError::Timeout) => {}

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::other("asset download stopped"));
            }
        }
    }
}

pub(crate) fn get(
    agent: &ureq::Agent,
    url: &str,
    options: &Options,
    started: Instant,
) -> io::Result<String> {
    options.check(started)?;
    tracing::debug!(%url, "Fetching asset");

    let bytes = agent
        .get(url)
        .config()
        .timeout_global(Some(options.timeout.saturating_sub(started.elapsed())))
        .build()
        .call()
        .map_err(io::Error::other)?
        .body_mut()
        .read_to_vec()
        .map_err(io::Error::other)?;

    options.check(started)?;
    tracing::debug!(%url, bytes = bytes.len(), "Asset received");

    String::from_utf8(bytes).map_err(invalid)
}

pub(crate) fn install(
    directory: &Path,
    files: BTreeMap<String, String>,
    options: &Options,
    started: Instant,
) -> io::Result<()> {
    options.check(started)?;

    if !files.contains_key("metadata.json") {
        return Err(invalid("missing asset metadata"));
    }

    tracing::debug!(path = %directory.display(), files = files.len(), "Installing asset bundle");

    let parent = directory
        .parent()
        .ok_or_else(|| invalid("asset directory has no parent"))?;

    fs::create_dir_all(parent)?;

    let staging = tempfile::tempdir_in(parent)?;

    let result = (|| {
        for (name, text) in files {
            options.check(started)?;
            fs::write(staging.path().join(name), text)?;
        }

        options.check(started)?;

        match fs::rename(staging.path(), directory) {
            Ok(()) => Ok(true),

            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::AlreadyExists
                        | io::ErrorKind::DirectoryNotEmpty
                        | io::ErrorKind::PermissionDenied
                ) && directory.join("metadata.json").is_file() =>
            {
                Ok(false)
            }

            Err(error) => Err(error),
        }
    })();

    let cleanup = if matches!(result, Ok(true)) {
        drop(staging.keep());

        Ok(())
    } else {
        staging.close()
    };

    result.map(|_| ()).and(cleanup)?;

    options.check(started)
}

pub(crate) fn revision(value: &str) -> io::Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid asset revision"));
    }

    Ok(())
}
