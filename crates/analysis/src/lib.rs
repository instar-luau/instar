//! Shared source analysis contracts.

mod diagnostic;
pub mod error;
mod limits;
mod location;
mod result;
pub mod selection;

pub use diagnostic::{Diagnostic, Kind};
pub use limits::{Cancellation, Completion, Options, Reason};
pub use location::{Location, Related};
pub use result::Result;
