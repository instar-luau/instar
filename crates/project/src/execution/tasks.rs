use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    task::{Context, Poll, Waker},
};

use instar_shared::{Event, OperationFailure, Options};

use super::scheduling::failure;

/// An awaitable project operation whose drop cancels queued or running child work.
pub struct Task<Output> {
    pub(crate) state: Arc<Mutex<TaskState<Output>>>,
    pub(crate) options: Options,
    pub(crate) events: Arc<Mutex<Vec<Event>>>,
}

pub(crate) struct TaskState<Output> {
    pub(crate) result: Option<Result<Output, OperationFailure>>,
    pub(crate) waker: Option<Waker>,
}

impl<Output> Future for Task<Output> {
    type Output = Result<Output, OperationFailure>;

    fn poll(self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);

        if let Some(result) = state.result.take() {
            Poll::Ready(result)
        } else if let Some(completion) = self.options.interrupted() {
            Poll::Ready(Err(failure(
                &completion.error(),
                &self.options,
                &self.events,
            )))
        } else {
            state.waker = Some(context.waker().clone());

            Poll::Pending
        }
    }
}

impl<Output> Drop for Task<Output> {
    fn drop(&mut self) {
        self.options.cancellation.cancel();
    }
}
