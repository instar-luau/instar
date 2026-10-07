//! Analysis input error construction.
use std::{fmt::Display, io};

use serde::{Deserialize, Serialize};

const KINDS: &[io::ErrorKind] = &[
    io::ErrorKind::NotFound,
    io::ErrorKind::PermissionDenied,
    io::ErrorKind::ConnectionRefused,
    io::ErrorKind::ConnectionReset,
    io::ErrorKind::HostUnreachable,
    io::ErrorKind::NetworkUnreachable,
    io::ErrorKind::ConnectionAborted,
    io::ErrorKind::NotConnected,
    io::ErrorKind::AddrInUse,
    io::ErrorKind::AddrNotAvailable,
    io::ErrorKind::NetworkDown,
    io::ErrorKind::BrokenPipe,
    io::ErrorKind::AlreadyExists,
    io::ErrorKind::WouldBlock,
    io::ErrorKind::NotADirectory,
    io::ErrorKind::IsADirectory,
    io::ErrorKind::DirectoryNotEmpty,
    io::ErrorKind::ReadOnlyFilesystem,
    io::ErrorKind::StaleNetworkFileHandle,
    io::ErrorKind::InvalidInput,
    io::ErrorKind::InvalidData,
    io::ErrorKind::TimedOut,
    io::ErrorKind::WriteZero,
    io::ErrorKind::StorageFull,
    io::ErrorKind::NotSeekable,
    io::ErrorKind::QuotaExceeded,
    io::ErrorKind::FileTooLarge,
    io::ErrorKind::ResourceBusy,
    io::ErrorKind::ExecutableFileBusy,
    io::ErrorKind::Deadlock,
    io::ErrorKind::CrossesDevices,
    io::ErrorKind::TooManyLinks,
    io::ErrorKind::InvalidFilename,
    io::ErrorKind::ArgumentListTooLong,
    io::ErrorKind::Interrupted,
    io::ErrorKind::Unsupported,
    io::ErrorKind::UnexpectedEof,
    io::ErrorKind::OutOfMemory,
    io::ErrorKind::Other,
];

/// An error crossing the local worker process boundary.
#[derive(Debug, Deserialize, Serialize)]
pub struct Failure {
    kind: String,
    code: Option<i32>,
    message: String,
}

impl From<io::Error> for Failure {
    fn from(error: io::Error) -> Self {
        Self {
            kind: format!("{:?}", error.kind()),
            code: error.raw_os_error(),
            message: error.to_string(),
        }
    }
}

impl TryFrom<Failure> for io::Error {
    type Error = io::Error;

    fn try_from(failure: Failure) -> Result<Self, Self::Error> {
        let kind = if let Some(code) = failure.code {
            let kind = Self::from_raw_os_error(code).kind();

            if format!("{kind:?}") != failure.kind {
                return Err(invalid(
                    "worker error category does not match its operating system code",
                ));
            }

            kind
        } else {
            KINDS
                .iter()
                .copied()
                .find(|kind| format!("{kind:?}") == failure.kind)
                .ok_or_else(|| invalid("unrecognized worker error category"))?
        };

        Ok(Self::new(kind, failure.message))
    }
}

/// Creates an invalid-data error while preserving its explanation.
pub fn invalid(error: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
