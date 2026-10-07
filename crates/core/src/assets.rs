use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use instar_analysis::{Options, error::invalid};
use sha2::{Digest, Sha256};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const HOST: &str = "https://instar-luau.github.io/instar";
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) fn directory(namespace: &str) -> io::Result<PathBuf> {
    let base = if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .filter(|path| Path::new(path).is_absolute())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
    }
    .ok_or_else(|| {
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
            Ok(result) => return result,
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

    String::from_utf8(bytes).map_err(invalid)
}

pub(crate) fn install(directory: &Path, mut files: BTreeMap<String, String>) -> io::Result<()> {
    let metadata = files
        .remove("metadata.json")
        .ok_or_else(|| invalid("missing asset metadata"))?;

    fs::create_dir_all(directory)?;

    let staging = directory.join(format!(
        ".download-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));

    fs::create_dir(&staging)?;

    let result = (|| {
        for (name, text) in files
            .into_iter()
            .chain([("metadata.json".to_owned(), metadata)])
        {
            let path = staging.join(&name);
            fs::write(&path, text)?;
            fs::rename(path, directory.join(name))?;
        }

        Ok(())
    })();

    let cleanup = fs::remove_dir_all(staging);

    result.and(cleanup)
}

pub(crate) fn revision(value: &str) -> io::Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid("invalid asset revision"));
    }

    Ok(())
}
