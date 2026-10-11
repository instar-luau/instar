use std::{
    io::{self, Read as _},
    time::{Duration, Instant},
};

use instar_shared::{
    Options, Reason,
    process::{Emitter, Outcome, Process, serve},
};

use serde::{Deserialize, Serialize};
use ureq::{Agent, Error as DownloadError, http::Uri};

use crate::assets::{self, MAXIMUM_BYTES};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadRequest {
    pub(crate) selection: String,
    pub(crate) expected_digest: String,
    pub(crate) remaining_budget: Option<Duration>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DownloadResult {
    pub(crate) selection: String,
    pub(crate) digest: String,
    pub(crate) contents: String,
}

#[derive(Default)]
pub(crate) struct Acquisition {
    process: Process<DownloadRequest, DownloadResult>,
}

impl Acquisition {
    pub(crate) fn acquire(
        &mut self,
        selection: &str,
        expected_digest: &str,
        options: &Options,
    ) -> io::Result<DownloadResult> {
        options.check()?;
        validate(selection, expected_digest)?;
        let requested_selection = selection.to_owned();
        let requested_digest = expected_digest.to_owned();

        let result = self.process.request_with(
            "project-assets",
            options,
            move |_, _, remaining_budget| DownloadRequest {
                selection: requested_selection,
                expected_digest: requested_digest,
                remaining_budget,
            },
        )?;

        match result {
            Outcome::Response(result) => {
                options.check()?;

                if result.selection != selection || result.digest != expected_digest {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "asset worker output does not match requested identity",
                    ));
                }

                if result.contents.len() > MAXIMUM_BYTES {
                    return Err(Reason::AnalysisLimit.error());
                }

                assets::verify(&result.contents, expected_digest)?;
                options.check()?;

                Ok(result)
            }

            Outcome::Interrupted(completion) => Err(completion.error()),
        }
    }

    pub(crate) fn finish(&mut self) -> io::Result<()> {
        self.process.finish()
    }
}

/// Serves isolated acquisition requests without installing any downloaded data.
///
/// # Errors
/// Returns framing, identity, download, interruption or digest-verification failures.
pub fn run_asset_worker() -> io::Result<()> {
    serve("project-assets", download)
}

fn download(request: DownloadRequest, _: &Emitter<DownloadResult>) -> io::Result<DownloadResult> {
    validate(&request.selection, &request.expected_digest)?;

    let deadline = request
        .remaining_budget
        .map(|budget| {
            Instant::now()
                .checked_add(budget)
                .ok_or_else(|| invalid("asset budget exceeds monotonic clock range"))
        })
        .transpose()?;

    let options = Options::new(deadline);
    options.check()?;

    let agent = Agent::config_builder()
        .https_only(true)
        .timeout_global(options.remaining())
        .build()
        .new_agent();

    let mut response = agent
        .get(&request.selection)
        .call()
        .map_err(|error| download_error(error, &options))?;

    options.check()?;
    let mut reader = response.body_mut().as_reader();
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];

    loop {
        options.check()?;

        let count = reader
            .read(&mut buffer)
            .map_err(|error| download_error(error.into(), &options))?;

        if count == 0 {
            break;
        }

        if bytes.len() + count > MAXIMUM_BYTES {
            return Err(Reason::AnalysisLimit.error());
        }

        bytes.extend_from_slice(&buffer[..count]);
    }

    options.check()?;

    let contents = String::from_utf8(bytes)
        .map_err(|error| invalid(&format!("downloaded assets are not UTF-8: {error}")))?;

    assets::verify(&contents, &request.expected_digest)?;
    options.check()?;

    Ok(DownloadResult {
        selection: request.selection,
        digest: request.expected_digest,
        contents,
    })
}

pub(crate) fn validate(selection: &str, digest: &str) -> io::Result<()> {
    let authority = selection
        .strip_prefix("https://")
        .and_then(|selection| selection.split('/').next())
        .ok_or_else(|| invalid("asset selection requires an HTTPS URL"))?;

    if authority.is_empty()
        || authority.contains('@')
        || selection
            .bytes()
            .any(|byte| byte.is_ascii_control() || byte.is_ascii_whitespace())
        || selection.contains(['\\', '?', '#'])
    {
        return Err(invalid(
            "asset selection contains unsupported URL components",
        ));
    }

    let identity: Uri = selection.parse().map_err(|error| {
        invalid(&format!(
            "asset selection requires a valid HTTPS identity: {error}"
        ))
    })?;

    if identity.scheme_str() != Some("https") || identity.host().is_none_or(str::is_empty) {
        return Err(invalid("asset selection requires a valid HTTPS identity"));
    }

    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid(
            "asset acquisition requires a canonical SHA-256 digest",
        ));
    }

    Ok(())
}

fn download_error(error: DownloadError, options: &Options) -> io::Error {
    if let Err(interruption) = options.check() {
        return interruption;
    }

    match error {
        DownloadError::Timeout(_) => {
            io::Error::new(io::ErrorKind::TimedOut, "asset download timed out")
        }

        DownloadError::Io(error) => error,
        _ => io::Error::other("asset download failed"),
    }
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
