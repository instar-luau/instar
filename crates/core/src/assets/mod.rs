//! Configuration schema and Luau environment asset generation and caching.

mod cache;
mod catalog;
mod content;
mod creator;
mod error;
mod generation;
mod mapping;
mod metadata;
mod publication;
mod reference;
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
