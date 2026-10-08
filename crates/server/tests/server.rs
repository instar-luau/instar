//! Protocol ordering, document synchronization, and worker lifecycle tests.

#[path = "../src/dispatch.rs"]
mod dispatch;

#[path = "../src/position.rs"]
mod position;

#[path = "../src/session.rs"]
mod session;

#[path = "../src/transport.rs"]
mod transport;

#[path = "session/workers.rs"]
mod workers;

#[path = "../src/workspace.rs"]
mod workspace;

#[path = "unit/position.rs"]
mod positions;

#[path = "unit/workspace.rs"]
mod workspaces;

pub use transport::{serve, stdio};
