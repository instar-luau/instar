//! Workspace discovery and project services.

mod discovery;
mod environment;
mod error;
mod host;
mod instances;
mod model;
mod paths;
mod selection;
mod source;
mod write;

pub use self::{
    discovery::configuration_files,
    error::Error,
    model::Workspace,
    selection::{Access, Purpose},
};
