use std::{
    io,
    process::{Child, ExitStatus},
    thread,
    time::Duration,
};

use super::Error;

pub(super) fn interrupted(cancelled: fn() -> bool) -> Result<(), Error> {
    if cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) fn wait(
    child: &mut Child,
    cancelled: fn() -> bool,
    failure: impl Fn(io::Error) -> Error,
) -> Result<ExitStatus, Error> {
    let result = loop {
        if cancelled() {
            break Err(Error::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(source) => break Err(failure(source)),
        }
    };

    if result.is_err() {
        let termination = child.kill();
        let reaping = child.wait();

        if let Err(source) = termination
            && source.kind() != io::ErrorKind::InvalidInput
        {
            return Err(failure(source));
        }

        reaping.map_err(failure)?;
    }

    result
}
