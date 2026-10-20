//! Validated, lossless Luau source formatting.

pub(crate) mod fragment;

mod directives;
mod engine;
mod error;
mod imports;
mod layout;
mod organize;
mod preservation;
mod range;
mod settings;
mod statements;

pub use self::{
    engine::format,
    error::{Error, Kind},
    organize::organize,
    range::range,
    settings::Prepared,
};

pub use crate::edit::Edit;
