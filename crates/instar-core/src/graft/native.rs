use super::{RESPONSE_LIMIT, Request};
use std::{
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
};

pub(super) fn entry(directory: &Path) -> PathBuf {
    directory.join(format!("graft{}", std::env::consts::EXE_SUFFIX))
}

pub(super) fn validate(entry: &Path) -> io::Result<()> {
    if cfg!(windows)
        && entry
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .is_none_or(|extension| {
                !extension.eq_ignore_ascii_case(std::env::consts::EXE_EXTENSION)
            })
    {
        return Err(io::Error::other("native graft entry must be an executable"));
    }

    Ok(())
}

pub(super) fn invoke(entry: &Path, request: &Request<'_>) -> io::Result<Vec<u8>> {
    let input = serde_json::to_vec(request).map_err(io::Error::other)?;

    let mut child = Command::new(entry)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");

    thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.write_all(&input));
        let mut output = Vec::new();

        let read = stdout
            .take((RESPONSE_LIMIT + 1) as u64)
            .read_to_end(&mut output);

        if read.is_err() || output.len() > RESPONSE_LIMIT {
            let _termination = child.kill();
        }

        let status = child.wait();

        let written = writer
            .join()
            .map_err(|_| io::Error::other("graft input writer panicked"))?;

        read?;

        if output.len() > RESPONSE_LIMIT {
            return Err(io::Error::other("graft response exceeds the payload limit"));
        }

        let status = status?;

        if !status.success() {
            return Err(io::Error::other(format!(
                "native graft exited with {status}"
            )));
        }

        written?;

        Ok(output)
    })
}
