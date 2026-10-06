//! Analysis input error construction.

use std::{fmt::Display, io};

/// Creates an invalid-data error while preserving its explanation.
pub fn invalid(error: impl Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}
