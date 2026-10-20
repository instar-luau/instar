use std::{
    io::{self, ErrorKind},
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use eyre::{Result, WrapErr as _};

pub(super) const INTERVAL: Duration = Duration::from_millis(100);

static REQUESTED: AtomicBool = AtomicBool::new(false);

pub(super) fn initialize() -> Result<()> {
    ctrlc::set_handler(|| REQUESTED.store(true, Ordering::Relaxed))
        .wrap_err("Could not initialize interrupt handling.")
}

pub(super) fn requested() -> bool {
    REQUESTED.load(Ordering::Relaxed)
}

pub(super) fn check() -> Result<()> {
    if requested() {
        Err(io::Error::from(ErrorKind::Interrupted).into())
    } else {
        Ok(())
    }
}
