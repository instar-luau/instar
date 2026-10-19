//! Protocol-neutral, tolerant source analysis for editor services.

mod color;
mod error;
mod model;
mod rename;
mod source;
mod validation;

pub use self::{
    color::color_presentations,
    error::Error,
    model::{
        Access, Analysis, Binding, BindingKind, Color, Edit, Fold, FoldKind, Import,
        ImportExpression, Link, Occurrence, RenameError, RenameTarget, Symbol, SymbolKind, Token,
        TokenKind,
    },
    source::analyze,
    validation::validate,
};
