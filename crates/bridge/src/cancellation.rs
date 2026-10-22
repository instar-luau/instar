use cxx::SharedPtr;

use crate::bridge::bindings;

/// A thread-safe handle to Luau's in-progress analysis cancellation token.
#[derive(Clone)]
pub struct Cancellation {
    token: SharedPtr<bindings::FrontendCancellationToken>,
}

impl Cancellation {
    pub(crate) fn new(token: SharedPtr<bindings::FrontendCancellationToken>) -> Self {
        Self { token }
    }

    /// Interrupt native constraint generation and solving on the owning thread.
    pub fn cancel(&self) {
        bindings::cancel(&self.token);
    }

    /// Clear cancellation before starting the next operation on the owning thread.
    pub fn reset(&self) {
        bindings::reset(&self.token);
    }
}
