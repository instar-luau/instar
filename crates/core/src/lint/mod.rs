//! Configured lint rules and validated source corrections.

mod analysis;
mod bindings;
mod catalog;
mod directives;
mod error;
mod finding;
mod fixes;
mod settings;
mod symbols;
mod syntax;

pub use self::{
    catalog::rules,
    directives::suppress,
    error::Error,
    finding::{Edit, Finding},
    fixes::apply,
    settings::Prepared,
    symbols::Symbols,
    syntax::lint,
};
