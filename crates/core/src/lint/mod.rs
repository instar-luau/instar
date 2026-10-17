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

pub use catalog::rules;
pub use directives::suppress;
pub use error::Error;
pub use finding::{Edit, Finding};
pub use fixes::apply;
pub use settings::Prepared;
pub use symbols::Symbols;
pub use syntax::lint;
