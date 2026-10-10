//! Lazy isolated workers with correlated framing, interruption and explicit cleanup.

mod protocol;
mod transport;

pub use protocol::{Emitter, Handshake, Message, RequestFrame, serve};
pub use transport::{Outcome, Process, set_host};
