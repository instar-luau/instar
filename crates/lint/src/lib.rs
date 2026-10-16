//! Configured lint rules and validated source corrections.

mod catalog;
mod directives;
mod fixes;
mod native;
mod result;
mod settings;
mod syntax;

pub use directives::suppress;
pub use fixes::apply;
pub use native::analyze;
pub use result::{Edit, Error, Finding};
pub use settings::Prepared;
pub use syntax::lint;
