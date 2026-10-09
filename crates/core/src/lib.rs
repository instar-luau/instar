//! Project orchestration for the Luau toolchain.

pub mod analysis;
pub(crate) mod assets;
mod checking;
pub mod configuration;
pub mod discovery;
pub mod documentation;
pub(crate) mod environment;
pub(crate) mod extract;
mod formatting;
pub mod graph;
mod hierarchy;
mod hints;
mod hover;
mod linting;
pub mod native;
mod navigation;
pub mod project;
mod references;
pub mod resolve;
pub mod roblox;
mod signatures;
pub mod snapshot;
pub mod source;
pub(crate) mod sourcemap;
pub mod structure;
pub mod symbols;
pub mod tokens;
