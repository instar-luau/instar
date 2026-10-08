//! Project orchestration for the Luau toolchain.

pub mod analysis;
pub(crate) mod assets;
mod checking;
pub mod configuration;
pub mod documentation;
pub(crate) mod environment;
pub(crate) mod extract;
mod formatting;
pub mod graph;
mod linting;
pub mod native;
pub mod project;
pub mod resolve;
pub mod roblox;
pub mod snapshot;
pub mod source;
pub(crate) mod sourcemap;
