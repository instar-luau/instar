//! Asset transport and publication tests against the private implementation.

#[path = "../src/lib.rs"]
mod implementation;

#[path = "roblox/mod.rs"]
mod tests;

pub use implementation::{
    analysis, configuration, documentation, graph, native, project, resolve, roblox, snapshot,
    source,
};

use implementation::{assets, environment, extract, sourcemap};
