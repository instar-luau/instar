//! Typed `instar.toml` configuration, schema generation, and layered loading.

pub mod checking;
pub mod environment;
pub mod formatting;
pub mod linting;
pub mod resolution;

mod loading;
mod model;
mod paths;
mod schema;
mod source;

pub use self::loading::{defaults, layer, load, resolve};
pub use self::model::{Configuration, Flag, Selection};
