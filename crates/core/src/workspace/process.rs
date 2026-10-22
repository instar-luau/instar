use std::{io, process::ExitStatus};

use process_wrap::tokio::ChildWrapper;
use tokio_util::sync::CancellationToken;

use super::Error;

pub(super) fn interrupted(is_cancelled: &dyn Fn() -> bool) -> Result<(), Error> {
    if is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

pub(super) async fn wait(
    child: &mut dyn ChildWrapper,
    cancellation: &CancellationToken,
    failure: impl Fn(io::Error) -> Error,
) -> Result<ExitStatus, Error> {
    let result = tokio::select! {
        biased;
        () = cancellation.cancelled() => Err(Error::Cancelled),
        status = child.inner_mut().wait() => status.map_err(&failure),
    };

    stop(child).await.map_err(failure)?;

    result
}

pub(super) async fn stop(child: &mut dyn ChildWrapper) -> io::Result<()> {
    let termination = child.start_kill();
    let reaping = child.wait().await;

    if let Err(source) = termination
        && source.kind() != io::ErrorKind::InvalidInput
    {
        return Err(source);
    }

    reaping?;

    Ok(())
}
