use std::{
    fs::{self, File, OpenOptions, TryLockError},
    io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

use super::{Error, Workspace, environment, paths, process};

const FILES: [&str; 7] = [
    "metadata.json",
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

    let version = env!("CARGO_PKG_VERSION");
    let directory = parent.join(version);

    if exists(&directory)? {
        return Ok(directory);
    }

    fs::create_dir_all(&parent).map_err(|source| filesystem(&parent, source))?;

    let lock_path = parent.join(format!("{version}.lock"));

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
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(50)),
            Err(TryLockError::Error(source)) => return Err(filesystem(&lock_path, source)),
        }
    }

    if exists(&directory)? {
        return Ok(directory);
    }

    let staging = tempfile::tempdir_in(&parent).map_err(|source| filesystem(&parent, source))?;
    let bundle = staging.path().join("bundle");
    fs::create_dir(&bundle).map_err(|source| filesystem(&bundle, source))?;
    let diagnostics = staging.path().join("diagnostics");

    for filename in FILES {
        process::interrupted(cancelled)?;

        let url = format!(
            "https://github.com/instar-luau/instar/releases/download/v{version}/roblox-{filename}"
        );

        download(&url, &bundle.join(filename), &diagnostics, cancelled)?;
    }

    environment::validate(&bundle, cancelled)?;
    process::interrupted(cancelled)?;
    fs::rename(&bundle, &directory).map_err(|source| filesystem(&directory, source))?;

    Ok(directory)
}

fn download(
    url: &str,
    destination: &Path,
    diagnostics: &Path,
    cancelled: fn() -> bool,
) -> Result<(), Error> {
    let failure = |source| Error::Download {
        url: url.to_owned(),
        source,
    };

    let diagnostic = File::create(diagnostics).map_err(|source| filesystem(diagnostics, source))?;

    let mut child = Command::new("curl")
        .args([
            "--disable",
            "--fail",
            "--silent",
            "--show-error",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "30",
            "--max-time",
            "300",
            "--output",
        ])
        .arg(destination)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(diagnostic)
        .spawn()
        .map_err(failure)?;

    let status = process::wait(&mut child, cancelled, failure)?;

    if !status.success() {
        let diagnostic = paths::read(diagnostics)?;

        return Err(failure(io::Error::other(format!(
            "curl exited with {status}: {}",
            diagnostic.trim()
        ))));
    }

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
