//! Luau source linting.

pub mod configuration;
mod diagnostic;
mod inventory;
mod result;
mod rule;
mod rules;
mod source;

pub use configuration::{Configuration, Level};
pub use diagnostic::{Diagnostic, Kind};
pub use instar_analysis::{Cancellation, Completion, Location, Options, Reason, Related};
pub use result::Result;
pub use rule::Rule;
pub use rules::lint;
pub use source::{Inference, Require, Source};
