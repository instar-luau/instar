//! Protocol ordering, document synchronization, and worker lifecycle tests.

#[path = "../src/actions.rs"]
mod actions;

#[path = "../src/completion.rs"]
mod completion;

#[path = "../src/diagnostics.rs"]
mod diagnostics;

#[path = "../src/dispatch.rs"]
mod dispatch;

#[path = "../src/edits.rs"]
mod edits;

#[path = "../src/formatting.rs"]
mod formatting;

#[path = "../src/hierarchy.rs"]
mod hierarchy;

#[path = "../src/hints.rs"]
mod hints;

#[path = "../src/hover.rs"]
mod hover;

#[path = "../src/links.rs"]
mod links;

#[path = "../src/location.rs"]
mod location;

#[path = "../src/markup.rs"]
mod markup;

#[path = "../src/movement.rs"]
mod movement;

#[path = "../src/navigation.rs"]
mod navigation;

#[path = "../src/position.rs"]
mod position;

#[path = "../src/queries.rs"]
mod queries;

#[path = "../src/references.rs"]
mod references;

#[path = "../src/rename.rs"]
mod rename;

#[path = "../src/session.rs"]
mod session;

#[path = "../src/signatures.rs"]
mod signatures;

#[path = "../src/structure.rs"]
mod structure;

#[path = "../src/symbols.rs"]
mod symbols;

#[path = "../src/tokens.rs"]
mod tokens;

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
