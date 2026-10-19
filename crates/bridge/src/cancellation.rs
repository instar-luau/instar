use std::sync::Arc;

use crate::bridge::bindings;

/// A thread-safe handle to Luau's in-progress analysis cancellation token.
#[derive(Clone)]
pub struct Cancellation {
    token: Arc<Token>,
}

struct Token {
    identifier: u64,
}

impl Cancellation {
    pub(crate) fn new(identifier: u64) -> Self {
        Self {
            token: Arc::new(Token { identifier }),
        }
    }

    /// Interrupt native constraint generation and solving on the owning thread.
    pub fn cancel(&self) {
        bindings::cancel(self.token.identifier);
    }

    /// Clear cancellation before starting the next operation on the owning thread.
    pub fn reset(&self) {
        bindings::reset(self.token.identifier);
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        bindings::release(self.identifier);
    }
}
