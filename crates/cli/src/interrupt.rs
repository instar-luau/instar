use std::{
    io::{self, ErrorKind},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use eyre::{Result, WrapErr as _};

pub(super) const INTERVAL: Duration = Duration::from_millis(100);

static IS_REQUESTED: AtomicBool = AtomicBool::new(false);

pub(super) fn initialize() -> Result<()> {
    ctrlc::set_handler(|| IS_REQUESTED.store(true, Ordering::Relaxed))
        .wrap_err("Could not initialize interrupt handling.")
}

pub(super) fn is_requested() -> bool {
    IS_REQUESTED.load(Ordering::Relaxed)
}

pub(super) fn check() -> Result<()> {
    if is_requested() {
        Err(io::Error::from(ErrorKind::Interrupted).into())
    } else {
        Ok(())
    }
}
