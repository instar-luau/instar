//! Workspace discovery and project services.

mod assets;
mod discovery;
mod environment;
mod error;
mod generation;
mod host;
mod imports;
mod instances;
mod model;
mod paths;
mod process;
mod selection;
mod source;
mod symbols;
mod write;

pub use self::{
    discovery::{configuration_files, is_configuration},
    error::Error,
    generation::Generators,
    instances::Sourcemap,
    model::Workspace,
    selection::{Access, Purpose},
    symbols::{InvalidSource, Origin, SymbolOutcome},
    write::replace,
};
