//! Configured lint rules and validated source corrections.

mod analysis;
mod bindings;
mod calls;
mod catalog;
mod context;
mod control;
mod directives;
mod error;
mod expression;
mod finding;
mod fixes;
mod layout;
mod settings;
mod symbols;
mod syntax;

pub use self::{
    analysis::lint,
    catalog::rules,
    error::{Error, Failure},
    finding::Finding,
    fixes::apply,
    settings::Prepared,
    symbols::Symbols,
};

pub use crate::edit::Edit;
