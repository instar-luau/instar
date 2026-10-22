use std::{
    io::{self, ErrorKind},
    sync::LazyLock,
    time::Duration,
};

use eyre::{Result, WrapErr as _};
use tokio_util::sync::CancellationToken;

pub(super) const INTERVAL: Duration = Duration::from_millis(100);

pub(super) static CANCELLATION: LazyLock<CancellationToken> = LazyLock::new(CancellationToken::new);

pub(super) fn initialize() -> Result<()> {
    ctrlc::set_handler(|| CANCELLATION.cancel())
        .wrap_err("Could not initialize interrupt handling.")
}

pub(super) fn is_requested() -> bool {
    CANCELLATION.is_cancelled()
}

pub(super) fn check() -> Result<()> {
    if is_requested() {
        Err(io::Error::from(ErrorKind::Interrupted).into())
    } else {
        Ok(())
    }
}
