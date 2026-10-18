//! Workspace discovery and project services.

mod assets;
mod discovery;
mod environment;
mod error;
mod generation;
mod host;
mod instances;
mod model;
mod paths;
mod process;
mod selection;
mod source;
mod write;

pub use self::{
    discovery::configuration_files,
    error::Error,
    generation::Generators,
    model::Workspace,
    selection::{Access, Purpose},
    write::replace,
};
