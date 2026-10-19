//! Configuration schema and Luau environment asset generation.

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

pub(crate) use self::metadata::{Documentation, Metadata, digest};
