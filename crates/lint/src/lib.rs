//! Luau source linting.

pub mod configuration;
mod diagnostic;
mod fixes;

#[expect(
    clippy::arbitrary_source_item_ordering,
    reason = "Rust requires the macro definition before its re-export"
)]
mod inventory;

mod result;
mod rule;
mod rules;
mod source;

pub use configuration::{Configuration, Level};
pub use diagnostic::{Diagnostic, Kind};
pub use fixes::fixes;
pub use instar_analysis::{Cancellation, Completion, Location, Options, Reason, Related};
pub use result::Result;
pub use rule::Rule;
pub use rules::lint;
pub use source::{Inference, Require, Source};
