//! Configured lint rules and validated source corrections.

mod analysis;
mod bindings;
mod catalog;
mod directives;
mod fixes;
mod result;
mod settings;
mod symbols;
mod syntax;

pub use catalog::rules;
pub use directives::suppress;
pub use fixes::apply;
pub use result::{Edit, Error, Finding};
pub use settings::Prepared;
pub use symbols::Symbols;
pub use syntax::lint;
