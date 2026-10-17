//! Typed `instar.toml` configuration, schema generation, and layered loading.

pub mod check;
pub mod environment;
pub mod format;
pub mod lint;
pub mod resolution;

mod load;
mod model;
mod paths;
mod patterns;
mod schema;
mod source;

pub use self::{
    load::{defaults, layer, load, resolve},
    model::{Configuration, Flag, Selection},
    patterns::Patterns,
    schema::schema,
};
