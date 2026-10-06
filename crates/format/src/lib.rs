//! Luau source formatting.

pub mod configuration;

mod diagnostic;
mod formatter;
mod result;

pub use configuration::Configuration;
pub use diagnostic::Diagnostic;
pub use formatter::format;
pub use result::Result;
