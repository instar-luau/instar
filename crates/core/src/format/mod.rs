//! Validated, lossless Luau source formatting.

mod directives;
mod engine;
mod error;
mod imports;
mod insertion;
mod layout;
mod organize;
mod preservation;
mod range;
mod settings;
mod statements;

pub use self::{
    engine::format,
    error::{Error, Kind},
    insertion::{Insertion, insertion},
    organize::organize,
    range::range,
    settings::Prepared,
};

pub use crate::edit::Edit;
