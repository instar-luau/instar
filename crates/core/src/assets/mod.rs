//! Configuration schema and Luau environment asset generation and caching.

mod cache;
mod content;
mod error;
mod generation;
mod mapping;
mod metadata;
mod publication;
mod rendering;
mod syntax;

pub use self::{
    cache::refresh,
    error::Error,
    generation::{Generated, Target, generate},
};

pub(crate) use self::{
    cache::{Cache, Namespace},
    metadata::{Capability, Metadata},
};
