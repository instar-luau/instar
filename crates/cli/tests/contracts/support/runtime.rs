use std::{
    future::Future,
    time::{Duration, Instant},
};

use instar_shared::Options;
use tokio::runtime::Builder;

pub(crate) fn complete<Operation: Future>(operation: Operation) -> Operation::Output {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("create the contract operation runtime")
        .block_on(operation)
}

pub(crate) fn options() -> Options {
    Options::new(Some(Instant::now() + Duration::from_secs(20)))
}
