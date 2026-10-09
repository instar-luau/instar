//! Language server for the Luau toolchain.

mod actions;
mod completion;
mod diagnostics;
mod dispatch;
mod edits;
mod formatting;
mod hierarchy;
mod hints;
mod hover;
mod links;
mod location;
mod markup;
mod movement;
mod navigation;
mod position;
mod queries;
mod references;
mod rename;
mod session;
mod signatures;
mod structure;
mod symbols;
mod tokens;
mod transport;
mod workers;
mod workspace;

pub use transport::{serve, stdio};
