//! Typed `instar.toml` configuration, schema generation, and layered loading.

pub mod check;
pub mod environment;
pub mod format;
pub mod lint;
mod load;
mod model;
mod paths;
pub mod resolution;
mod schema;
mod source;

pub use load::{defaults, layer, load, resolve};
pub use model::{Configuration, Flag, Selection};
