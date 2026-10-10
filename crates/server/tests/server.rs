//! Protocol ordering, synchronized revisions, and project task lifecycle tests.

#[path = "unit/workspace.rs"]
mod documents;

#[path = "../src/edits.rs"]
mod edits;

#[path = "../src/handlers/mod.rs"]
mod handlers;

#[path = "../src/location.rs"]
mod location;

#[path = "../src/markup.rs"]
mod markup;

#[path = "../src/position.rs"]
mod position;

#[path = "unit/position.rs"]
mod positions;

#[path = "session/protocol.rs"]
mod protocol;

#[path = "../src/session/mod.rs"]
mod session;

#[path = "../src/transport/mod.rs"]
mod transport;

pub use session::Server;
pub use transport::{ServerOutcome, serve, stdio};
