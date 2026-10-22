//! Configuration schema and Luau environment asset generation.

mod content;
mod error;
mod generation;
mod mapping;
mod metadata;
mod publication;
mod rendering;
mod syntax;

pub use self::{
    error::Error,
    generation::{Generated, Target, generate},
};

pub(crate) use self::metadata::{Capability, Metadata};
