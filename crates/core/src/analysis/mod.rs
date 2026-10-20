//! Protocol-neutral, tolerant source analysis for editor services.

mod color;
mod error;
mod model;
mod rename;
mod source;
mod validation;

pub use self::{
    color::{Color, color_presentations},
    error::Error,
    model::{
        Access, Analysis, Binding, BindingKind, Edit, Fold, FoldKind, Import, ImportExpression,
        Link, Occurrence, Symbol, SymbolKind, Token, TokenKind,
    },
    rename::{RenameError, RenameTarget},
    source::analyze,
    validation::validate,
};

pub use crate::syntax::is_identifier;
